pub mod generated;

mod gdt;
mod regs;

pub use eqgate_microvm::layout;
pub use eqgate_microvm::layout::*;

use std::cmp::max;
use std::fs::File;

use axerrno::{AxResult, ax_err, ax_err_type};
use linux_loader::configurator::linux::LinuxBootConfigurator;
// use linux_loader::configurator::pvh::PvhBootConfigurator;
use linux_loader::configurator::{BootConfigurator, BootParams};
use linux_loader::loader::bootparam::boot_params;
use linux_loader::loader::elf::Elf as Loader;
use linux_loader::loader::{Cmdline, KernelLoader, PvhBootCapability, load_cmdline};

#[allow(unused)]
use crate::microvm::acpi::create_acpi_tables;
use crate::microvm::config::MachineConfig;
use crate::microvm::initrd::InitrdConfig;
use crate::microvm::mptable;
use crate::microvm::vstate::memory::{
    Address, GuestAddress, GuestMemory, GuestMemoryMmap, GuestMemoryRegion, GuestRegionType,
};
use crate::microvm::vstate::vm::Vm;

// Value taken from https://elixir.bootlin.com/linux/v5.10.68/source/arch/x86/include/uapi/asm/e820.h#L31
// Usable normal RAM
const E820_RAM: u32 = 1;

// Reserved area that should be avoided during memory allocations
const E820_RESERVED: u32 = 2;
// const MEMMAP_TYPE_RAM: u32 = 1;

/// Errors thrown while configuring x86_64 system.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
#[allow(unused)]
pub enum ConfigurationError {
    /// Invalid e820 setup params.
    E820Configuration,
    /// Error writing MP table to memory: {0}
    MpTableSetup(#[from] mptable::MptableError),
    /// Error writing the zero page of guest memory.
    ZeroPageSetup,
    /// Error writing module entry to guest memory.
    ModlistSetup,
    /// Error writing memory map table to guest memory.
    MemmapTableSetup,
    /// Error writing hvm_start_info to guest memory.
    StartInfoSetup,
    /// Cannot copy kernel file fd
    KernelFile,
    /// Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}
    KernelLoader(linux_loader::loader::Error),
    /// Cannot load command line string: {0}
    LoadCommandline(linux_loader::loader::Error),
    /// Cannot create kernel command line C string: {0}
    CommandLineCString(String),
    /// Invalid Linux boot parameter value: {0}
    BootParamValue(&'static str),
    // /// Failed to create guest config: {0}
    // CreateGuestConfig(#[from] GuestConfigError),
    /// Error configuring the vcpu for boot
    VcpuConfigure,
    /// Error configuring ACPI: {0}
    Acpi(#[from] crate::microvm::acpi::AcpiError),
}

pub use eqgate_microvm::BootProtocol;

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
    crate::microvm::arch::layout::HIMEM_START
}

/// Configures the system for booting Linux.
#[allow(clippy::too_many_arguments)]
pub fn configure_system_for_boot(
    vm: &Vm,
    _machine_config: &MachineConfig,
    entry_point: EntryPoint,
    initrd: &Option<InitrdConfig>,
    boot_cmdline: Cmdline,
) -> Result<(), ConfigurationError> {
    configure(vm.guest_memory(), entry_point)?;

    // Write the kernel command line to guest memory. This is x86_64 specific, since on
    // aarch64 the command line will be specified through the FDT.
    let cmdline_cstring = boot_cmdline
        .as_cstring()
        .map_err(|err| ConfigurationError::CommandLineCString(format!("{:?}", err)))?;
    let cmdline_size = cmdline_cstring.as_bytes_with_nul().len();

    warn!(
        "[LOG] Kernel cmdline size {:#x} Bytes, cmdline: {:?}",
        cmdline_size, cmdline_cstring
    );

    load_cmdline(
        vm.guest_memory(),
        GuestAddress(crate::microvm::arch::layout::CMDLINE_START),
        &boot_cmdline,
    )
    .map_err(ConfigurationError::LoadCommandline)?;

    // Put the MP table in Linux's legacy scan range (top 1 KiB of base 640 KiB RAM).
    // This ensures smp_found_config is set even without ACPI tables.
    // mptable::setup_mptable(
    //     vm.guest_memory(),
    //     &mut vm.resource_allocator(),
    //     machine_config.vcpu_count,
    // )
    // .map_err(ConfigurationError::MpTableSetup)?;

    match entry_point.protocol {
        BootProtocol::PvhBoot => {
            // configure_pvh(vm.guest_memory(), GuestAddress(CMDLINE_START), initrd)?;
            unimplemented!("PVH boot protocol is not yet supported in EquationOS microVM");
        }
        BootProtocol::LinuxBoot => {
            configure_64bit_boot(
                vm.guest_memory(),
                GuestAddress(CMDLINE_START),
                cmdline_size,
                initrd,
            )?;
        }
    }

    // Create ACPI tables and write them in guest memory
    // For the time being we only support ACPI in x86_64
    // create_acpi_tables(
    //     vm.guest_memory(),
    //     &mut vm.resource_allocator(),
    //     machine_config.max_vcpu_count(),
    // )?;
    Ok(())
}

fn configure_64bit_boot(
    guest_mem: &GuestMemoryMmap,
    cmdline_addr: GuestAddress,
    cmdline_size: usize,
    initrd: &Option<InitrdConfig>,
) -> Result<(), ConfigurationError> {
    warn!(
        "[LOG] Configuring 64-bit Linux boot, cmdline addr {:#x} size {:#x} Bytes",
        cmdline_addr.raw_value(),
        cmdline_size
    );

    const KERNEL_BOOT_FLAG_MAGIC: u16 = 0xaa55;
    const KERNEL_HDR_MAGIC: u32 = 0x5372_6448;
    const KERNEL_LOADER_OTHER: u8 = 0xff;
    const KERNEL_MIN_ALIGNMENT_BYTES: u32 = 0x0100_0000; // Must be non-zero.

    let himem_start = GuestAddress(layout::HIMEM_START);

    warn!("[LOG] HIMEM_START at {:#x}", himem_start.raw_value());

    // Set the location of RSDP in Boot Parameters to help the guest kernel find it faster.
    let mut params = boot_params {
        acpi_rsdp_addr: layout::RSDP_ADDR,
        ..Default::default()
    };

    params.hdr.type_of_loader = KERNEL_LOADER_OTHER;
    params.hdr.boot_flag = KERNEL_BOOT_FLAG_MAGIC;
    params.hdr.header = KERNEL_HDR_MAGIC;
    params.hdr.cmd_line_ptr = u32::try_from(cmdline_addr.raw_value())
        .map_err(|_| ConfigurationError::BootParamValue("cmd_line_ptr"))?;
    params.hdr.cmdline_size = u32::try_from(cmdline_size)
        .map_err(|_| ConfigurationError::BootParamValue("cmdline_size"))?;
    params.hdr.kernel_alignment = KERNEL_MIN_ALIGNMENT_BYTES;
    if let Some(initrd_config) = initrd {
        params.hdr.ramdisk_image = u32::try_from(initrd_config.address.raw_value())
            .map_err(|_| ConfigurationError::BootParamValue("ramdisk_image"))?;
        params.hdr.ramdisk_size = u32::try_from(initrd_config.size)
            .map_err(|_| ConfigurationError::BootParamValue("ramdisk_size"))?;
    }

    // We mark first [0x0, SYSTEM_MEM_START) region as usable RAM and the subsequent
    // [SYSTEM_MEM_START, (SYSTEM_MEM_START + SYSTEM_MEM_SIZE)) as reserved (note
    // SYSTEM_MEM_SIZE + SYSTEM_MEM_SIZE == HIMEM_START).
    add_e820_entry(&mut params, 0, layout::SYSTEM_MEM_START, E820_RAM)?;
    add_e820_entry(
        &mut params,
        layout::SYSTEM_MEM_START,
        layout::SYSTEM_MEM_SIZE,
        E820_RESERVED,
    )?;
    add_e820_entry(
        &mut params,
        PCI_MMCONFIG_START,
        PCI_MMCONFIG_SIZE,
        E820_RESERVED,
    )?;

    for region in guest_mem
        .iter()
        .filter(|region| region.region_type == GuestRegionType::Dram)
    {
        // the first 1MB is reserved for the kernel
        let addr = max(himem_start, region.start_addr());
        add_e820_entry(
            &mut params,
            addr.raw_value(),
            region.last_addr().unchecked_offset_from(addr) + 1,
            E820_RAM,
        )?;
    }

    LinuxBootConfigurator::write_bootparams(
        &BootParams::new(&params, GuestAddress(layout::ZERO_PAGE_START)),
        guest_mem,
    )
    .map_err(|_| ConfigurationError::ZeroPageSetup)
}

/// Add an e820 region to the e820 map.
/// Returns Ok(()) if successful, or an error if there is no space left in the map.
fn add_e820_entry(
    params: &mut boot_params,
    addr: u64,
    size: u64,
    mem_type: u32,
) -> Result<(), ConfigurationError> {
    if params.e820_entries as usize >= params.e820_table.len() {
        return Err(ConfigurationError::E820Configuration);
    }

    params.e820_table[params.e820_entries as usize].addr = addr;
    params.e820_table[params.e820_entries as usize].size = size;
    params.e820_table[params.e820_entries as usize].type_ = mem_type;
    params.e820_entries += 1;

    Ok(())
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

pub fn configure(
    guest_mem: &GuestMemoryMmap,
    kernel_entry_point: EntryPoint,
) -> Result<(), ConfigurationError> {
    regs::setup_sregs(guest_mem, kernel_entry_point.protocol).map_err(|e| {
        error!("Failed to setup special registers: {:#x?}", e);
        ConfigurationError::VcpuConfigure
    })
}
