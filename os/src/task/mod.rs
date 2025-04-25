//! Task management implementation
//!
//! Everything about task management, like starting and switching tasks is
//! implemented here.
//!
//! A single global instance of [`TaskManager`] called `TASK_MANAGER` controls
//! all the tasks in the operating system.
//!
//! Be careful when you see `__switch` ASM function in `switch.S`. Control flow around this function
//! might not be what you expect.

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::loader::{get_app_data, get_num_app};
use crate::sync::UPSafeCell;
use crate::trap::TrapContext;
use alloc::vec::Vec;
use lazy_static::*;
use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};
use crate::config::MAX_SYSCALL_ID;
pub use context::TaskContext;
use crate::mm::{MapPermission, PTEFlags, PhysAddr, VirtAddr};

/// The task manager, where all the tasks are managed.
///
/// Functions implemented on `TaskManager` deals with all task state transitions
/// and task context switching. For convenience, you can find wrappers around it
/// in the module level.
///
/// Most of `TaskManager` are hidden behind the field `inner`, to defer
/// borrowing checks to runtime. You can see examples on how to use `inner` in
/// existing functions on `TaskManager`.
pub struct TaskManager {
    /// total number of tasks
    num_app: usize,
    /// use inner value to get mutable access
    inner: UPSafeCell<TaskManagerInner>,
}

/// The task manager inner in 'UPSafeCell'
struct TaskManagerInner {
    /// task list
    tasks: Vec<TaskControlBlock>,
    /// id of current `Running` task
    current_task: usize,
    syscall_cnt:[usize;MAX_SYSCALL_ID]
}

lazy_static! {
    /// a `TaskManager` global instance through lazy_static!
    pub static ref TASK_MANAGER: TaskManager = {
        println!("init TASK_MANAGER");
        let num_app = get_num_app();
        println!("num_app = {}", num_app);
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..num_app {
            tasks.push(TaskControlBlock::new(get_app_data(i), i));
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                    syscall_cnt: [0;MAX_SYSCALL_ID]
                })
            },
        }
    };
}

impl TaskManager {
    /// Run the first task in task list.
    ///
    /// Generally, the first task in task list is an idle task (we call it zero process later).
    /// But in ch4, we load apps statically, so the first task is a real app.
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let next_task = &mut inner.tasks[0];
        next_task.task_status = TaskStatus::Running;
        let next_task_cx_ptr = &next_task.task_cx as *const TaskContext;
        drop(inner);
        let mut _unused = TaskContext::zero_init();
        // before this, we should drop local variables that must be dropped manually
        unsafe {
            __switch(&mut _unused as *mut _, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    /// Change the status of current `Running` task into `Ready`.
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].task_status = TaskStatus::Ready;
    }

    /// Change the status of current `Running` task into `Exited`.
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].task_status = TaskStatus::Exited;
    }

    /// Find next task to run and return task id.
    ///
    /// In this case, we only return the first `Ready` task in task list.
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.num_app + 1)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Get the current 'Running' task's token.
    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_user_token()
    }

    /// Get the current 'Running' task's trap contexts.
    fn get_current_trap_cx(&self) -> &'static mut TrapContext {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_trap_cx()
    }

    /// Change the current 'Running' task's program break
    pub fn change_current_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].change_program_brk(size)
    }

    /// Switch current `Running` task to the task we have found,
    /// or there is no `Ready` task and we can exit with all applications completed
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            drop(inner);
            // before this, we should drop local variables that must be dropped manually
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // go back to user mode
        } else {
            panic!("All applications completed!");
        }
    }

    /// get &mut T by virtual addr in current memory set
    pub fn get_mut_ref<T>(&self, vaddr:VirtAddr) -> Option<&'static mut T> {
        let bit38 = vaddr.0 >> 38 & 0x1;
        let top_bits = vaddr.0 >> 39;
        let inner = self.inner.exclusive_access();
        if (bit38 ==0 && top_bits ==0) || (bit38 == 1&& !top_bits == 0){
            if let Some(pte) = inner.tasks[inner.current_task]
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
        let pte = inner.tasks[inner.current_task]
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
    fn inc_syscall(&self, id: usize){
        let mut inner = self.inner.exclusive_access();
        if id >= MAX_SYSCALL_ID {
            panic!("syscall id out of range!");
        } else {
            inner.syscall_cnt[id] += 1;
        }
    }

    /// Return syscall counter with specify id
    fn get_syscall_cnt(&self, id:usize) -> usize{
        let inner = self.inner.exclusive_access();
        if id >= MAX_SYSCALL_ID {
            panic!("syscall id out of range!");
        } else{
            inner.syscall_cnt[id]
        }
    }

    /// Clear syscall counter by zero
    fn clear_syscall(&self){
        let mut inner = self.inner.exclusive_access();
        inner.syscall_cnt.iter_mut().for_each(|c|*c = 0);
    }

    /// map a framed area to current memory set
    fn map_frame(&self, va: VirtAddr, ve: VirtAddr, perm:MapPermission) -> bool {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].memory_set.map_frame(
            va, ve, perm
        )
    }

    fn unmap_frame(&self, va: VirtAddr, ve: VirtAddr) -> bool{
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].memory_set.unmap_frame(
            va, ve
        )
    }

    fn debug_vaddr(&self, va: VirtAddr) {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].memory_set.translate_debug(va.floor());
    }
}

/// Run the first task in task list.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Switch current `Running` task to the task we have found,
/// or there is no `Ready` task and we can exit with all applications completed
fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

/// Change the status of current `Running` task into `Ready`.
fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

/// Change the status of current `Running` task into `Exited`.
fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

/// Suspend the current 'Running' task and run the next task in task list.
pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}

/// Get the current 'Running' task's token.
pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

/// Get the current 'Running' task's trap contexts.
pub fn current_trap_cx() -> &'static mut TrapContext {
    TASK_MANAGER.get_current_trap_cx()
}

/// Change the current 'Running' task's program break
pub fn change_program_brk(size: i32) -> Option<usize> {
    TASK_MANAGER.change_current_program_brk(size)
}

/// get &mut T by virtual addr in current memory set
pub fn get_mut_ref<T>(vaddr:VirtAddr)-> Option<&'static mut T> {
    TASK_MANAGER.get_mut_ref(vaddr)
}

/// get &T by virtual addr in current memory set
pub fn get_ref<T>(vaddr:VirtAddr)-> Option<&'static T> {
    TASK_MANAGER.get_ref(vaddr)
}

/// increment the syscall counter by 1
pub fn inc_syscall(id:usize){
    TASK_MANAGER.inc_syscall(id);
}

/// get the syscall counter with specified id
pub fn get_syscall_cnt(id:usize) -> usize{
    TASK_MANAGER.get_syscall_cnt(id)
}

/// clear current syscall counter
pub fn clear_syscall(){
    TASK_MANAGER.clear_syscall();
}

/// map a framed area to current memory set
pub fn map_frame(va: VirtAddr, ve: VirtAddr, perm:MapPermission) -> bool{
    TASK_MANAGER.map_frame(va, ve, perm)
}

/// map a or some framed area to current memory set
pub fn unmap_frame(va: VirtAddr, ve: VirtAddr) -> bool{
    TASK_MANAGER.unmap_frame(va, ve)
}

/// show a virtual addr translation path used for debug
#[allow(unused)]
pub fn debug_vaddr(vaddr: VirtAddr){
    TASK_MANAGER.debug_vaddr(vaddr);
}