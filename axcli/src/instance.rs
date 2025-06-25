use libc::c_void;

use crate::hvc::{hvc_create_instance, hvc_init_shim};
use crate::shared_pages::{copy_content_to_shared_pages, free_shared_pages};
use crate::{ExecuteArgs, InstanceCreateArgs};

pub fn create_instance(args: InstanceCreateArgs) {
    info!(
        "Create {} instance with file path: {:?}",
        match args.instance_type {
            0 => "LibOS",
            1 => "Kernel",
            _ => panic!("Invalid instance type"),
        },
        args.file_path
    );

    let raw_file = std::fs::read(args.file_path).expect("Failed to read ELF file");

    // Copy the raw file to axvisor through shared pages
    // page by page.
    let mut shared_pages: Vec<*mut c_void> = Vec::new();
    copy_content_to_shared_pages(&mut shared_pages, raw_file.as_slice());

    let iid = hvc_create_instance(
        args.instance_type as _,
        if args.one2onemapping { 0 } else { 1 } as _,
        raw_file.len() as _,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as _,
    );

    if iid < 0 {
        panic!("Failed to create instance: {}", iid);
    }

    info!("Create instance success, instance ID = [{}]", iid);

    free_shared_pages(&mut shared_pages);
}

pub fn load_junction(args: ExecuteArgs) {
    use pi_memory_layout::{ArgsLayoutBuilder, ArgsLayoutRef};

    // We need to copy execution metadate to axvisor to start the loader instance.
    let mut args_builder = ArgsLayoutBuilder::new();
    for arg in &args.exec_args {
        args_builder.add_argv(arg);
    }
    args_builder.add_envv("EQTEST=1"); // Set EQTEST environment variable
    let args_layout = args_builder.build();

    if false {
        // Print the stack layout for debugging purposes
        let layout = ArgsLayoutRef::new(args_layout.as_ref(), None);

        for (i, arg) in unsafe { layout.argv_iter() }.enumerate() {
            println!("  [{i}] {}", arg.to_str().unwrap());
        }
        for (i, env) in unsafe { layout.envv_iter() }.enumerate() {
            println!("  [env {i}] {}", env.to_str().unwrap());
        }
    }

    // Copy the stack layout to axvisor through shared pages
    // page by page.
    let mut shared_pages: Vec<*mut c_void> = Vec::new();
    copy_content_to_shared_pages(&mut shared_pages, args_layout.as_ref());

    let iid = hvc_create_instance(
        1, // 1 for dynamic loading instance
        1, // 1 for CoarseGrainedSegmentation2M
        args_layout.len() as _,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as _,
    );
    info!("Create instance success, instance ID = [{}]", iid);

    free_shared_pages(&mut shared_pages);

    // At this point, we just passed the stack layout (the arguments and environment variables)
    // to the axvisor, and the axvisor will handle the loading of the junction.
    // In the next step, this process will turn into a proxy process of the junction instance,
    // which handles the system calls which can not be handled by the axvisor directly.
}

pub fn init_shim() {
    info!("Init shim");
    hvc_init_shim();
}
