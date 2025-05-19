use crate::elf::copy_file;
use crate::hvc::hvc_init_shim;
use crate::InstanceCreateArgs;

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


    copy_file(&args.file_path.unwrap());
}

pub fn init_shim() {
    info!("Init shim");
    hvc_init_shim();
}
