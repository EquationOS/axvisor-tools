//! MicroVM related functionalities.
//! Most of the code here is adapted from Firecracker's microVM design.

mod acpi;
pub mod arch;
mod config;
mod initrd;
mod mptable;
mod resource;
mod vstate;

use arch::layout;

use std::fs;

use crate::MicroVMCreateArgs;

use arch::configure_system_for_boot;
use arch::load_kernel;
use axerrno::{AxResult, ax_err_type};
use initrd::InitrdConfig;
use resource::VmResources;
use vstate::vm::Vm;

pub fn create_microvm(args: MicroVMCreateArgs) -> AxResult {
    info!(
        "Create Linux instance with config file path: {:?}",
        args.config_file
    );

    let config_json = fs::read_to_string(args.config_file)
        .expect("Unable to open or read from the configuration file");

    let vm_resources = VmResources::from_json(&config_json).expect("Failed to build VM resources");

    let boot_config = vm_resources.boot_source.builder.as_ref().ok_or_else(|| {
        ax_err_type!(
            InvalidInput,
            "Boot source builder is missing in the VM resources"
        )
    })?;

    let guest_memory = vm_resources
        .allocate_guest_memory()
        .expect("Failed to allocate guest memory");

    // Clone the command-line so that a failed boot doesn't pollute the original.
    #[allow(unused_mut)]
    let mut boot_cmdline = boot_config.cmdline.clone();

    let mut vm = Vm::new(vm_resources.fd).expect("Failed to create VM instance");

    vm.register_dram_memory_regions(guest_memory)?;

    let entry_point = load_kernel(&boot_config.kernel_file, vm.guest_memory())?;

    info!(
        "Kernel loaded at entry point: {:#x?}, boot protocol: {:?}",
        entry_point.entry_addr, entry_point.protocol
    );

    let initrd = InitrdConfig::from_config(boot_config, vm.guest_memory())?;

    configure_system_for_boot(
        &vm,
        &vm_resources.machine_config,
        entry_point,
        &initrd,
        boot_cmdline,
    )
    .map_err(|e| ax_err_type!(BadState, format_args!("configuration error {}", e)))?;

    crate::hvc::hvc_microvm_boot(vm_resources.vm_id as u64);

    Ok(())
}
