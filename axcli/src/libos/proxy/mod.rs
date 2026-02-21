pub mod daemon;

mod page_cache;
mod scf;
mod syscall;

use std::{collections::BTreeMap, sync::Mutex};

static FD_LIST: Mutex<BTreeMap<i32, String>> = Mutex::new(BTreeMap::new());

static mut INSTANCE_FD: i32 = -1;
static mut INSTANCE_ID: usize = 0;

fn get_instance_fd() -> i32 {
    unsafe {
        if INSTANCE_FD <= 0 {
            panic!("INSTANCE_FD is not set, please call setup_proxy_daemon first");
        }
        INSTANCE_FD
    }
}

fn instance_id() -> usize {
    unsafe {
        if INSTANCE_ID == 0 {
            panic!("INSTANCE_ID is not set, please call setup_proxy_daemon first");
        }
        INSTANCE_ID
    }
}

pub fn setup_proxy_daemon(instance_id: usize, instance_fd: i32) {
    // Set the global INSTANCE_FD to the instance fd.
    unsafe {
        INSTANCE_ID = instance_id;
        INSTANCE_FD = instance_fd;
    }

    // Setup the syscall queue buffer for SCF (system call forwarding).
    scf::setup_syscall_proxy_queue_buffer();

    // Setup the page cache region for the instance.
    page_cache::setup_shared_page_cache_region();
}
