use std::ffi::CStr;

use equation_defs::gate::region::KSCHED_SHM_REGION_SIZE;
use libc::c_void;

use pi_memory_layout::{ArgsLayoutBuilder, ArgsLayoutRef};

use crate::hvc::{hvc_init_shim, hvc_setup_instance};
use crate::ioctl::EQINSTANCE_DEV_PREFIX;
use crate::libos::ExecuteArgs;
use crate::libos::proxy;
use crate::libos::shared_pages::{copy_content_to_shared_pages, free_shared_pages};

/// Remote execute in a instance setup by AxVisor.
pub fn execute(args: ExecuteArgs) {
    // First, create the instance through ioctl, eqdriver will trigger the hvc to create the instance.
    let instance_id =
        crate::ioctl::ioctl_create_libos().expect("Failed to create instance for dynamic loading");

    info!("Create instance success, instance ID = [{}]", instance_id);

    let instance_dev_path_str = format!("{}{}\0", EQINSTANCE_DEV_PREFIX, instance_id);
    let instance_dev_path = CStr::from_bytes_with_nul(instance_dev_path_str.as_bytes())
        .expect("Failed to create CStr for instance device path");

    let instance_fd = unsafe {
        libc::open(
            instance_dev_path.as_ptr() as *const libc::c_char,
            libc::O_RDWR,
        )
    };

    if instance_fd < 0 {
        error!(
            "Failed to open instance device {:?}: {}",
            instance_dev_path,
            std::io::Error::last_os_error()
        );
        return;
    }

    // We need to copy execution metadate to axvisor to start the loader instance.
    let mut args_builder = ArgsLayoutBuilder::new();
    // Arguments, arg[0] is the executable file.
    for arg in &args.exec_args {
        args_builder.add_argv(arg);
    }
    // Set EQTEST environment variable
    args_builder.add_envv("EQTEST=1");

    let args_layout = args_builder.build();

    if true {
        // Print the stack layout for debugging purposes
        let layout = ArgsLayoutRef::new(args_layout.as_ref(), None);

        for (i, arg) in unsafe { layout.argv_iter() }.enumerate() {
            println!("  [{i}] {}", arg.to_str().unwrap());
        }
        for (i, env) in unsafe { layout.envv_iter() }.enumerate() {
            println!("  [env {i}] {}", env.to_str().unwrap());
        }
        for auxv in unsafe { layout.auxv_iter() } {
            println!("  [auxv] {:?}", auxv);
        }
    }

    // Copy the stack layout to axvisor through shared pages
    // page by page.
    let mut shared_pages: Vec<*mut c_void> = Vec::new();
    copy_content_to_shared_pages(&mut shared_pages, args_layout.as_ref());

    let res = hvc_setup_instance(
        instance_id as _,
        args_layout.len() as u64,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as u64,
    );
    if res < 0 {
        panic!("Failed to setup instance: {}", res);
    }

    info!("Setup instance success, instance ID = [{}]", instance_id);

    free_shared_pages(&mut shared_pages);

    proxy::setup_proxy_daemon(instance_id, instance_fd);

    // In the next step, this process will turn into a daemon proxy process of the junction instance,
    // which handles the system calls which can not be handled by the axvisor directly.
    proxy::daemon::poll();
}

pub fn remove_instance(instance_id: u64) {
    info!("Remove instance with ID: {}", instance_id);

    crate::ioctl::ioctl_remove_instance(instance_id).expect("Failed to remove instance");

    info!("Instance with ID {} removed successfully", instance_id);
}

pub fn init_shim() {
    const KSCHED_DEV_PATH_STR: &str = "/dev/ksched\0";
    let ksched_dev_path = CStr::from_bytes_with_nul(KSCHED_DEV_PATH_STR.as_bytes())
        .expect("Failed to create CStr for ksched device path");

    let ksched_fd = unsafe {
        libc::open(
            ksched_dev_path.as_ptr() as *const libc::c_char,
            libc::O_RDWR,
        )
    };

    if ksched_fd < 0 {
        error!(
            "Failed to open ksched device {:?}: {}",
            ksched_dev_path,
            std::io::Error::last_os_error()
        );
        return;
    }
    let ksched_shm_base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            KSCHED_SHM_REGION_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            ksched_fd,
            0,
        )
    };

    info!("Init shim, ksched_shm_base: 0x{:#?}", ksched_shm_base);
    hvc_init_shim(ksched_shm_base as _);
}
