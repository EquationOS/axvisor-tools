//! MicroVM related functionalities.
//! Most of the code here is adapted from Firecracker's microVM design.

#[allow(unused)]
mod acpi;
pub mod arch;
mod cli;
mod config;
pub(crate) mod console;
pub(crate) mod control;
mod initrd;
#[allow(unused)]
mod mptable;
mod resource;
mod vfio_runtime;
mod vstate;

pub use cli::*;
pub use config::IovaMode;
pub use config::PciBdf;
pub use resource::VfioResourceConfig;

use std::fs;
use std::thread;
use std::time::Duration;

use crate::hvc::hvc_init_shim;
use cli::MicroVMCreateArgs;

use arch::configure_system_for_boot;
use arch::load_kernel;
use axerrno::{AxResult, ax_err_type};
use initrd::InitrdConfig;
use resource::VmResources;
use vfio_runtime::{run_foreground_daemon_loop, setup_vfio_dma_holder};
use vstate::vm::Vm;

pub fn init_gate() {
    // For now, we just reuse the init arg of LibOS instance.
    // We use arg == 0 to indicate the MicroVM gate instance.
    hvc_init_shim(0);
}

pub fn create_microvm(args: MicroVMCreateArgs) -> AxResult {
    info!(
        "Create Linux instance with config file path: {:?}",
        args.config_file
    );

    let config_json = fs::read_to_string(args.config_file)
        .expect("Unable to open or read from the configuration file");

    // Build microVM resources from the configuration file.
    // It will create the microVM instance in the kernel via ioctl and get an instance ID, which is used as the VM ID for later interactions with this microVM.
    // This includes preparing the VM configuration, allocating guest memory, and setting up VFIO DMA mappings if needed.
    let vm_resources = VmResources::from_json(&config_json).expect("Failed to build VM resources");

    if !vm_resources.passthrough_devices.is_empty() {
        for bdf in &vm_resources.passthrough_devices {
            warn!("microVM passthrough-device requested: {}", bdf.format());
        }
        warn!(
            "BDF list is now forwarded through ioctl/metadata to EqVisor microVM backend. \
If devices are still not visible in guest, complete BAR/interrupt mapping is likely missing."
        );
    }
    if let Some(vfio) = vm_resources.vfio {
        warn!(
            "microVM vfio requested: iommu-group={} guest-visible-bdf={:04x}:{:02x}:{:02x}.{} iova-mode={:?}",
            vfio.iommu_group,
            vfio.guest_visible_bdf.domain,
            vfio.guest_visible_bdf.bus,
            vfio.guest_visible_bdf.device,
            vfio.guest_visible_bdf.function,
            vfio.iova_mode
        );
    }

    let boot_config = vm_resources.boot_source.builder.as_ref().ok_or_else(|| {
        ax_err_type!(
            InvalidInput,
            "Boot source builder is missing in the VM resources"
        )
    })?;

    let guest_memory = vm_resources
        .allocate_guest_memory()
        .expect("Failed to allocate guest memory");

    let keep_foreground = vm_resources.vfio.is_some() || vm_resources.microvm_console_ring_gpa != 0;
    // Build persistent VFIO DMA mappings in current process before guest boot.
    // In daemon mode this process itself keeps VFIO fds/mappings alive.
    setup_vfio_dma_holder(&vm_resources, &guest_memory)?;

    // Clone the command-line so that a failed boot doesn't pollute the original.
    #[allow(unused_mut)]
    let mut boot_cmdline = boot_config.cmdline.clone();
    let default_vcpus = vm_resources.machine_config.default_vcpu_count();
    let max_vcpus = vm_resources.machine_config.max_vcpu_count();
    if max_vcpus > default_vcpus {
        boot_cmdline
            .insert("maxcpus".to_string(), default_vcpus.to_string())
            .map_err(|e| {
                ax_err_type!(InvalidInput, format_args!("maxcpus cmdline error: {}", e))
            })?;
        boot_cmdline
            .insert("nr_cpus".to_string(), max_vcpus.to_string())
            .map_err(|e| {
                ax_err_type!(InvalidInput, format_args!("nr_cpus cmdline error: {}", e))
            })?;
        info!(
            "microVM CPU elasticity enabled: default_vcpus={} max_vcpus={} appended maxcpus={} nr_cpus={}",
            default_vcpus, max_vcpus, default_vcpus, max_vcpus
        );
    }

    let mut vm = Vm::new(vm_resources.fd).expect("Failed to create VM instance");
    console::attach_console(
        vm_resources.fd,
        vm_resources.vm_id,
        vm_resources.microvm_console_ring_gpa,
    )
    .map_err(|e| ax_err_type!(BadState, format_args!("console attach error {}", e)))?;
    control::start_control_socket(
        vm_resources.fd,
        vm_resources.vm_id,
        default_vcpus,
        max_vcpus,
    )
    .map_err(|e| ax_err_type!(BadState, format_args!("control socket error {}", e)))?;

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

    crate::hvc::hvc_microvm_boot(
        vm_resources.vm_id as u64,
        entry_point.entry_addr.0,
        entry_point.protocol as u8,
    );

    if keep_foreground {
        if vm_resources.vfio.is_some() {
            run_foreground_daemon_loop();
        } else {
            run_console_control_loop();
        }
    }

    Ok(())
}

fn run_console_control_loop() -> ! {
    loop {
        console::poll_console_once();
        control::poll_control_once();
        thread::sleep(Duration::from_millis(10));
    }
}
