//! Process management syscalls

use crate::mm::{MapPermission, VirtAddr};
use crate::task::{change_program_brk, exit_current_and_run_next, suspend_current_and_run_next,
                  get_ref, get_mut_ref, get_syscall_cnt, map_frame, unmap_frame};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();

    if let Some(p) = get_mut_ref(VirtAddr::from(ts as usize)) {
        *p = TimeVal{
            sec:us / 1_000_000,
            usec:us % 1_000_000,
        };
        0
    } else{
        -1
    }
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        0 => {
            if let Some(p) = get_ref::<i8>(VirtAddr(id)) {
                *p as _
            } else {
                -1
            }
        },
        1 => {
            if let Some(p) = get_mut_ref(id.into()) {
                *p = data as u8;
                0
            } else {
                -1
            }
        },
        2 => {
            get_syscall_cnt(id) as _
        },
        _ => -1
    }
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    let vaddr_start = VirtAddr::from(start);

    let vpn_end = VirtAddr::from(start + len).ceil();
    if !vaddr_start.aligned() ||
        port & !0x7 !=0 ||
        port & 0x7 == 0{
        -1
    } else{
        if map_frame(vaddr_start, vpn_end.into(),
                     MapPermission::from_bits((port << 1) as u8 | 1<<4).unwrap()){
            0
        } else{
            -1
        }
    }

}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    if unmap_frame(VirtAddr::from(start), VirtAddr::from(start + len)){
        0
    } else{
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
