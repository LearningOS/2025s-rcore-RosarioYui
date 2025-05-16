//! Types related to task management & Functions for completely changing TCB
use super::{TaskContext};
use super::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
use crate::config::{TRAP_CONTEXT_BASE, BIG_STRIDE};
use crate::mm::{MemorySet, PhysPageNum, VirtAddr, KERNEL_SPACE, MapPermission, PTEFlags, PhysAddr};
use crate::sync::UPSafeCell;
use crate::trap::{trap_handler, TrapContext};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::RefMut;
use crate::config::MAX_SYSCALL_ID;
use core::cmp::Ordering;

/// Task control block structure
///
/// Directly save the contents that will not change during running
pub struct TaskControlBlock {
    // Immutable
    /// Process identifier
    pub pid: PidHandle,

    /// Kernel stack corresponding to PID
    pub kernel_stack: KernelStack,

    /// Mutable
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    /// Get the mutable reference of the inner TCB
    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// Get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        let inner = self.inner_exclusive_access();
        inner.memory_set.token()
    }
}


impl PartialEq for TaskControlBlock {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl PartialOrd for TaskControlBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for TaskControlBlock {}

impl Ord for TaskControlBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        let stride_a = self.inner.exclusive_access().get_stride();
        let stride_b = other.inner.exclusive_access().get_stride();

        (BIG_STRIDE>>1).cmp(&(stride_a.0.wrapping_sub(stride_b.0)))
    }
}

#[derive(Clone, Copy, Debug)]
struct Stride(u8);

pub struct TaskControlBlockInner {
    /// The physical page number of the frame where the trap context is placed
    pub trap_cx_ppn: PhysPageNum,

    /// Application data can only appear in areas
    /// where the application address space is lower than base_size
    pub base_size: usize,

    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// Parent process of the current process.
    /// Weak will not affect the reference count of the parent
    pub parent: Option<Weak<TaskControlBlock>>,

    /// A vector containing TCBs of all child processes of the current process
    pub children: Vec<Arc<TaskControlBlock>>,

    /// It is set when active exit or execution error occurs
    pub exit_code: i32,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,

    /// counter for syscall
    syscall_cnt:[usize;MAX_SYSCALL_ID],

    /// current running time
    stride: Stride,

    /// priority
    pass: u8
}

impl TaskControlBlockInner {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
    pub fn is_zombie(&self) -> bool {
        self.get_status() == TaskStatus::Zombie
    }
    fn get_stride(&self) -> Stride {
        self.stride
    }
}

impl TaskControlBlock {
    /// Create a new process
    ///
    /// At present, it is only used for the creation of initproc
    pub fn new(elf_data: &[u8]) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        // push a task context which goes to trap_return to the top of kernel stack
        let task_control_block = Self {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: user_sp,
                    program_brk: user_sp,
                    syscall_cnt: [0; MAX_SYSCALL_ID],
                    stride: Stride(0),
                    pass: BIG_STRIDE >> 4 // default priority is 16
                })
            },
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }

    /// set schedule priority
    pub fn set_prio(&self, priority:isize) -> isize{
        let mut inner = self.inner_exclusive_access();
        if priority >= 2 {
            // overflow
            inner.pass = BIG_STRIDE / priority as u8;
            priority as isize
        } else{
            -1
        }
    }

    /// update current running time
    pub fn update_stride(&self){
        let mut inner = self.inner_exclusive_access();
        inner.stride.0 = inner.stride.0.wrapping_add(inner.pass);
    }

    /// Load a new elf to replace the original application address space and start execution
    pub fn exec(&self, elf_data: &[u8]) {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();

        // **** access current TCB exclusively
        let mut inner = self.inner_exclusive_access();
        // substitute memory_set
        inner.memory_set = memory_set;
        // update trap_cx ppn
        inner.trap_cx_ppn = trap_cx_ppn;
        // initialize base_size
        inner.base_size = user_sp;
        // initialize trap_cx
        let trap_cx = inner.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            self.kernel_stack.get_top(),
            trap_handler as usize,
        );
        // **** release inner automatically
    }

    /// parent process fork the child process
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        // ---- access parent PCB exclusively
        let mut parent_inner = self.inner_exclusive_access();
        // copy user space(include trap context)
        let memory_set = MemorySet::from_existed_user(&parent_inner.memory_set);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        let task_control_block = Arc::new(TaskControlBlock {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: parent_inner.base_size,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: parent_inner.heap_bottom,
                    program_brk: parent_inner.program_brk,
                    syscall_cnt: parent_inner.syscall_cnt,
                    stride: Stride(0),
                    pass: BIG_STRIDE >> 4
                })
            },
        });
        // add child
        parent_inner.children.push(task_control_block.clone());
        // modify kernel_sp in trap_cx
        // **** access child PCB exclusively
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        trap_cx.kernel_sp = kernel_stack_top;
        // return
        task_control_block
        // **** release child PCB
        // ---- release parent PCB
    }

    /// parent process spawn  the child process
    pub fn spawn(self: &Arc<Self>, elf_data: &[u8]) -> Arc<Self>{
        let new_task = Arc::new(TaskControlBlock::new(elf_data));
        let mut parent_inner = self.inner_exclusive_access();
        parent_inner.children.push(new_task.clone());
        new_task.inner_exclusive_access().parent = Some(Arc::downgrade(&self));
        new_task
    }

    /// get pid of process
    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner_exclusive_access();
        let heap_bottom = inner.heap_bottom;
        let old_break = inner.program_brk;
        let new_brk = inner.program_brk as isize + size as isize;
        if new_brk < heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            inner
                .memory_set
                .shrink_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        } else {
            inner
                .memory_set
                .append_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            inner.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }

    /// get &mut T by virtual addr in current memory set
    pub fn get_mut_ref<T>(&self, vaddr:VirtAddr) -> Option<&'static mut T> {
        let bit38 = vaddr.0 >> 38 & 0x1;
        let top_bits = vaddr.0 >> 39;
        let inner = self.inner.exclusive_access();
        if (bit38 ==0 && top_bits ==0) || (bit38 == 1&& !top_bits == 0){
            if let Some(pte) = inner
                .memory_set
                .translate(vaddr.floor()){
                if pte.flags().contains(PTEFlags::W | PTEFlags::R){
                    let mut phy_addr:usize = PhysAddr::from(pte.ppn()).into();
                    phy_addr += vaddr.page_offset();
                    unsafe {
                        (phy_addr as *mut T).as_mut()
                    }
                } else{
                    None
                }
            } else{
                None
            }
        } else{
            None
        }
    }

    /// get &T by virtual addr in current memory set
    pub fn get_ref<T>(&self, vaddr:VirtAddr) -> Option<&'static T> {
        let bit38 = vaddr.0 >> 38 & 0x1;
        let top_bits = vaddr.0 >> 39;
        let inner = self.inner.exclusive_access();
        if (bit38 == 0 && top_bits != 0) || (bit38 == 1 && top_bits != (1 << 25) - 1) {
            return None;
        }
        let pte = inner
                .memory_set
                .translate(vaddr.floor())?;

        if pte.flags().contains(PTEFlags::R) {
            let mut phy_addr: usize = PhysAddr::from(pte.ppn()).into();
            phy_addr += vaddr.page_offset();
            unsafe {
                (phy_addr as *mut T).as_ref()
            }
        } else{
            None
        }
    }

    /// Increment syscall counter with specify id
    pub fn inc_syscall(&self, id: usize){
        let mut inner = self.inner.exclusive_access();
        if id >= MAX_SYSCALL_ID {
            panic!("syscall id out of range!");
        } else {
            inner.syscall_cnt[id] += 1;
        }
    }

    /// Return syscall counter with specify id
    pub fn get_syscall_cnt(&self, id:usize) -> usize{
        let inner = self.inner.exclusive_access();
        if id >= MAX_SYSCALL_ID {
            panic!("syscall id out of range!");
        } else{
            inner.syscall_cnt[id]
        }
    }

    /// Clear syscall counter by zero
    pub fn clear_syscall(&self){
        let mut inner = self.inner.exclusive_access();
        inner.syscall_cnt.iter_mut().for_each(|c|*c = 0);
    }

    /// map a framed area in current memory set
    pub fn map_frame(&self, va: VirtAddr, ve: VirtAddr, perm:MapPermission) -> bool {
        let mut inner = self.inner.exclusive_access();
        inner.memory_set.map_frame(
            va, ve, perm
        )
    }

    /// unmap a framed area in current memory set
    pub fn unmap_frame(&self, va: VirtAddr, ve: VirtAddr) -> bool{
        let mut inner = self.inner.exclusive_access();
        inner.memory_set.unmap_frame(
            va, ve
        )
    }

    /// show vaddr transition path for debug
    pub fn debug_vaddr(&self, va: VirtAddr) {
        let inner = self.inner.exclusive_access();
        inner.memory_set.translate_debug(va.floor());
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Zombie,
}
