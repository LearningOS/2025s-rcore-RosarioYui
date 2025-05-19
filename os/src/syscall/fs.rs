//! File and filesystem-related syscalls
use core::any::Any;
use crate::fs::{open_file, OSInode, OpenFlags, Stat, StatMode, inode_count, create_link, remove_link};
use crate::mm::{translated_byte_buffer, translated_refmut, translated_str, UserBuffer};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// YOUR JOB: Implement fstat.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    trace!(
        "kernel:pid[{}] sys_fstat",
        current_task().unwrap().pid.0
    );
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        trace!("fstat error: fd overflow");
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        trace!("fstat error: fd not allocated");
        return -1;
    }
    if let Some(fd_item) = &inner.fd_table[fd]{
        if let Some(os_inode) = (fd_item as &dyn Any).downcast_ref::<OSInode>() {
            let k_st = translated_refmut(inner.get_user_token(), st);
            let tmp = os_inode.inode();
            let id = tmp.inode_id();

            let link_count = inode_count(id);
            *k_st = Stat::new(0,
                        id as _,
                        if os_inode.inode().is_dir(){
                            StatMode::DIR
                        } else{
                            StatMode::FILE
                        },
                        link_count as _
            );

            0
        } else{
            trace!("fstat error: type error");
            -1
        }

    } else{
        trace!("fstat error: fd error-2");
        -1
    }
}

/// YOUR JOB: Implement linkat.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_linkat",
        current_task().unwrap().pid.0
    );
    let old_name = translated_str(current_user_token(), old_name);
    let new_name = translated_str(current_user_token(), new_name);
    if let Some(old_os_inode) = open_file(old_name.as_str(), OpenFlags::RDONLY){
        let old_inode = old_os_inode.inode();
        // update root dir entry
        if create_link(old_inode.inode_id(), new_name.as_str()){
            0
        } else{
            -1
        }
    } else{
        -1
    }
}

/// YOUR JOB: Implement unlinkat.
pub fn sys_unlinkat(name: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_unlinkat",
        current_task().unwrap().pid.0
    );
    let name = translated_str(current_user_token(), name);
    if let Some(os_inode) = open_file(name.as_str(), OpenFlags::RDONLY){
        let inode = os_inode.inode();
        if inode_count(inode.inode_id()) == 1{
            inode.clear();
        }
        if remove_link(name.as_str()){
            0
        } else{
            -1
        }
    } else{
        -1
    }
}
