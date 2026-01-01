//! MicroVM related functionalities.
//! Most of the code here is adapted from Firecracker's microVM design.

mod config;
mod image;
mod layout;
mod memory;
mod resource;
mod vm;

use std::fs;

use crate::hvc::hvc_create_instance;

use crate::InstanceCreateArgs;

use axerrno::ax_err_type;
use config::{BootSource, GuestConfig};
use image::load_kernel;
use resource::VmResources;
use vm::Vm;

pub fn create_microvm(args: InstanceCreateArgs) {
    info!(
        "Create Linux instance with config file path: {:?}",
        args.config_file
    );

    let config_json = fs::read_to_string(args.config_file)
        .expect("Unable to open or read from the configuration file");

    let guest_config = serde_json::from_str::<GuestConfig>(&config_json)
        .expect("Failed to parse json config file");

    let init_vcpu_count = guest_config.machine_config.as_ref().map(|mc| mc.vcpu_count);

    let vm_resources = VmResources::from_json(&config_json).expect("Failed to build VM resources");

    let boot_config = vm_resources
        .boot_source
        .builder
        .as_ref()
        .ok_or(ax_err_type!(
            InvalidInput,
            "Boot source builder is missing in the VM resources"
        ))
        .unwrap();

    let guest_memory = vm_resources
        .allocate_guest_memory()
        .expect("Failed to allocate guest memory");

    let mut vm = Vm::new(vm_resources.fd).expect("Failed to create VM instance");

    vm.register_dram_memory_regions(guest_memory)
        .expect("Failed to register guest memory");

    let entry_point = load_kernel(&boot_config.kernel_file, vm.guest_memory())
        .expect("Failed to load kernel image");

    info!(
        "Kernel loaded at entry point: {:#x?}, boot protocol: {:?}",
        entry_point.entry_addr, entry_point.protocol
    );
}
