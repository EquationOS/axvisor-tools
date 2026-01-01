use core::fmt;
use std::fs::File;

use axerrno::{AxResult, ax_err, ax_err_type};
use linux_loader::configurator::linux::LinuxBootConfigurator;
use linux_loader::configurator::pvh::PvhBootConfigurator;
use linux_loader::configurator::{BootConfigurator, BootParams};
use linux_loader::loader::bootparam::boot_params;
use linux_loader::loader::elf::Elf as Loader;
use linux_loader::loader::elf::start_info::{
    hvm_memmap_table_entry, hvm_modlist_entry, hvm_start_info,
};
use linux_loader::loader::{Cmdline, KernelLoader, PvhBootCapability, load_cmdline};

use crate::microvm::memory::{
    Address, GuestAddress, GuestMemory, GuestMemoryMmap, GuestMemoryRegion, GuestRegionType,
};

/// Supported boot protocols for
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BootProtocol {
    /// Linux 64-bit boot protocol
    LinuxBoot,
    #[cfg(target_arch = "x86_64")]
    /// PVH boot protocol (x86/HVM direct boot ABI)
    PvhBoot,
}

impl fmt::Display for BootProtocol {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        match self {
            BootProtocol::LinuxBoot => write!(f, "Linux 64-bit boot protocol"),
            #[cfg(target_arch = "x86_64")]
            BootProtocol::PvhBoot => write!(f, "PVH boot protocol"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
/// Specifies the entry point address where the guest must start
/// executing code, as well as which boot protocol is to be used
/// to configure the guest initial state.
pub struct EntryPoint {
    /// Address in guest memory where the guest must start execution
    pub entry_addr: GuestAddress,
    /// Specifies which boot protocol to use
    pub protocol: BootProtocol,
}

/// Returns the memory address where the kernel could be loaded.
pub fn get_kernel_start() -> u64 {
    crate::microvm::layout::HIMEM_START
}

/// Load linux kernel into guest memory.
pub fn load_kernel(kernel: &File, guest_memory: &GuestMemoryMmap) -> AxResult<EntryPoint> {
    // Need to clone the File because reading from it
    // mutates it.
    let mut kernel_file = kernel.try_clone().map_err(|e| {
        ax_err_type!(
            InvalidData,
            format_args!("failed to clone kernel file handle: {}", e)
        )
    })?;

    let entry_addr = Loader::load(
        guest_memory,
        None,
        &mut kernel_file,
        Some(GuestAddress(get_kernel_start())),
    )
    .map_err(|e| {
        ax_err_type!(
            InvalidData,
            format_args!("failed to load kernel into guest memory: {}", e)
        )
    })?;

    let entry_point_addr: GuestAddress = entry_addr.kernel_load;
    let boot_prot: BootProtocol = BootProtocol::LinuxBoot;
    if let PvhBootCapability::PvhEntryPresent(pvh_entry_addr) = entry_addr.pvh_boot_cap {
        // Use the PVH kernel entry point to boot the guest
        // entry_point_addr = pvh_entry_addr;
        // boot_prot = BootProtocol::PvhBoot;
        error!(
            "Found entry addr at {:#x?}. PVH boot protocol is not supported in this build",
            pvh_entry_addr
        );

        return ax_err!(
            Unsupported,
            "PVH boot protocol is not supported in this build"
        );
    }

    debug!("Kernel pvh_boot_cap: {:#x?}", entry_addr.pvh_boot_cap);

    debug!(
        "Kernel loaded using {boot_prot}, entry addr: {:#x?}",
        entry_point_addr
    );

    Ok(EntryPoint {
        entry_addr: entry_point_addr,
        protocol: boot_prot,
    })
}
