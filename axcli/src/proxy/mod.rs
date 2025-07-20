pub mod daemon;

mod fs;
mod misc;
mod net;
mod shm;

#[allow(unused)]
mod mm;

mod page_cache;
mod scf;

use std::{collections::BTreeMap, sync::Mutex};

static FD_LIST: Mutex<BTreeMap<i32, String>> = Mutex::new(BTreeMap::new());

static mut INSTANCE_FD: i32 = -1;

fn get_instance_fd() -> i32 {
    unsafe {
        if INSTANCE_FD <= 0 {
            panic!("INSTANCE_FD is not set, please call setup_syscall_proxy_queue_buffer first");
        }
        INSTANCE_FD
    }
}

pub fn setup_proxy_daemon(instance_fd: i32) {
    // Set the global INSTANCE_FD to the instance fd.
    unsafe {
        INSTANCE_FD = instance_fd;
    }

    // Setup the syscall queue buffer for SCF (system call forwarding).
    scf::setup_syscall_proxy_queue_buffer();

    // Setup the page cache region for the instance.
    page_cache::setup_shared_page_cache_region();
}
