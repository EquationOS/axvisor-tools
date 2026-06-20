use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use axerrno::{AxResult, ax_err_type};

use crate::ioctl;
use crate::microvm::resource::VmResources;
use crate::microvm::vstate::memory::{
    Address, GuestMemoryRegion, GuestRegionMmap, MemoryRegionAddress,
};

const VFIO_TYPE: u32 = b';' as u32;
const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

const VFIO_GET_API_VERSION_NR: u32 = 100;
const VFIO_CHECK_EXTENSION_NR: u32 = 101;
const VFIO_SET_IOMMU_NR: u32 = 102;
const VFIO_GROUP_GET_STATUS_NR: u32 = 103;
const VFIO_GROUP_SET_CONTAINER_NR: u32 = 104;
const VFIO_GROUP_GET_DEVICE_FD_NR: u32 = 106;
const VFIO_DEVICE_GET_REGION_INFO_NR: u32 = 108;
const VFIO_DEVICE_GET_IRQ_INFO_NR: u32 = 109;
const VFIO_DEVICE_SET_IRQS_NR: u32 = 110;
const VFIO_IOMMU_MAP_DMA_NR: u32 = 113;

const VFIO_GROUP_FLAGS_VIABLE: u32 = 1 << 0;
const VFIO_TYPE1_IOMMU: i32 = 1;
const VFIO_DMA_MAP_FLAG_READ: u32 = 1 << 0;
const VFIO_DMA_MAP_FLAG_WRITE: u32 = 1 << 1;
const VFIO_PCI_CONFIG_REGION_INDEX: u32 = 7;
const VFIO_PCI_BAR0_REGION_INDEX: u32 = 0;
const PCI_COMMAND_REG_OFFSET: u64 = 0x04;
const PCI_COMMAND_MEMORY: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_STATUS_REG_OFFSET: u64 = 0x06;
const PCI_CAP_PTR_OFFSET: u64 = 0x34;
const PCI_CAP_ID_PM: u8 = 0x01;
const PCI_CAP_ID_MSIX: u8 = 0x11;
const PCI_PMCSR_OFFSET_IN_CAP: u64 = 0x04;
const PCI_MSIX_FLAGS_OFFSET_IN_CAP: u64 = 0x02;
const PCI_MSIX_FLAGS_MASKALL: u16 = 1 << 14;
const PCI_MSIX_FLAGS_ENABLE: u16 = 1 << 15;
const VFIO_PCI_MSIX_IRQ_INDEX: u32 = 2;
const VFIO_IRQ_SET_DATA_EVENTFD: u32 = 1 << 2;
const VFIO_IRQ_SET_ACTION_TRIGGER: u32 = 1 << 5;
const MAX_MSIX_EVENT_FDS: usize = 64;
const PCI_MSIX_TABLE_ENTRY_SIZE: u64 = 16;
const MLX5_INIT_SEG_CMDQ_ADDR_H_OFF: u64 = 0x10;
const MLX5_INIT_SEG_CMDQ_ADDR_L_SZ_OFF: u64 = 0x14;
const MLX5_INIT_SEG_CMD_DBELL_OFF: u64 = 0x18;

#[repr(C)]
struct VfioGroupStatus {
    argsz: u32,
    flags: u32,
}

#[derive(Debug)]
#[repr(C)]
struct VfioIommuType1DmaMap {
    argsz: u32,
    flags: u32,
    vaddr: u64,
    iova: u64,
    size: u64,
}

#[repr(C)]
struct VfioRegionInfo {
    argsz: u32,
    flags: u32,
    index: u32,
    cap_offset: u32,
    size: u64,
    offset: u64,
}

#[repr(C)]
struct VfioIrqInfo {
    argsz: u32,
    flags: u32,
    index: u32,
    count: u32,
}

#[repr(C)]
struct VfioIrqSetHeader {
    argsz: u32,
    flags: u32,
    index: u32,
    start: u32,
    count: u32,
}

#[derive(Clone, Copy)]
struct VfioRuntimeState {
    container_fd: i32,
    group_fd: i32,
    device_fd: i32,
    instance_fd: i32,
    instance_id: usize,
    msix_event_count: usize,
    msix_event_fds: [i32; MAX_MSIX_EVENT_FDS],
    msix_ctrl_off: Option<u64>,
    msix_table_bir: Option<u32>,
    msix_table_offset: u64,
    msix_table_size: usize,
    cfg_region_offset: u64,
    cfg_region_size: u64,
    bar0_region_offset: u64,
    bar0_region_size: u64,
    guest_ram_iova_base: u64,
    guest_ram_vaddr_base: u64,
    guest_ram_size: u64,
}

static VFIO_RUNTIME_STATE: OnceLock<VfioRuntimeState> = OnceLock::new();
static VFIO_MSIX_CTRL_LOGGED: OnceLock<()> = OnceLock::new();
static VFIO_MLX5_BAR0_LAST: Mutex<Option<(u32, u32, u32)>> = Mutex::new(None);
static VFIO_CMDQ_LAST_SIG: Mutex<Option<(u64, u32)>> = Mutex::new(None);
static VFIO_MSIX_VECTOR_SHADOW: RwLock<[Option<u8>; MAX_MSIX_EVENT_FDS]> =
    RwLock::new([None; MAX_MSIX_EVENT_FDS]);
static VFIO_MSIX_POSTED_ROUTE_ACTIVE: RwLock<[bool; MAX_MSIX_EVENT_FDS]> =
    RwLock::new([false; MAX_MSIX_EVENT_FDS]);
static VFIO_CMDQ_TRACE_WARNED: AtomicBool = AtomicBool::new(false);
static VFIO_ACTIVE_ROUTE_FALLBACK_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
const VFIO_ACTIVE_ROUTE_FALLBACK_TRACE_LIMIT: usize = 64;

const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> u64 {
    ((dir as u64) << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

const fn io(ty: u32, nr: u32) -> u64 {
    ioc(0, ty, nr, 0)
}

const fn iow<T>(ty: u32, nr: u32) -> u64 {
    ioc(1, ty, nr, core::mem::size_of::<T>())
}

fn ioctl_ret(fd: i32, req: u64, arg: usize, what: &str) -> AxResult<i32> {
    let ret = unsafe { libc::ioctl(fd, req as libc::c_ulong, arg) };
    if ret < 0 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO ioctl {} failed: {}",
                what,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(ret as i32)
}

fn open_rdwr(path: &str) -> AxResult<i32> {
    let cpath = CString::new(path).map_err(|_| {
        ax_err_type!(
            InvalidInput,
            format_args!("Invalid path for CString: {}", path)
        )
    })?;
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "Failed to open {}: {}",
                path,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(fd)
}

fn setup_vfio_fds(resources: &VmResources) -> AxResult<(i32, i32, i32)> {
    let vfio = resources
        .vfio
        .ok_or_else(|| ax_err_type!(InvalidInput, "VFIO runtime requested without vfio config"))?;
    let host_bdf = resources
        .passthrough_devices
        .first()
        .ok_or_else(|| ax_err_type!(InvalidInput, "No passthrough device configured"))?;
    let bdf_str = host_bdf.format();

    let container_fd = open_rdwr("/dev/vfio/vfio")?;
    let api_version = ioctl_ret(
        container_fd,
        io(VFIO_TYPE, VFIO_GET_API_VERSION_NR),
        0,
        "VFIO_GET_API_VERSION",
    )?;
    // Kernel VFIO API version can be 0 (VFIO_API_VERSION == 0) on many hosts.
    // Treat negative return as failure (already handled in ioctl_ret), and only
    // log the returned value for diagnostics.
    if api_version < 0 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!("Unexpected VFIO API version: {}", api_version)
        ));
    }
    info!("VFIO API version reported by kernel: {}", api_version);

    let ext = ioctl_ret(
        container_fd,
        io(VFIO_TYPE, VFIO_CHECK_EXTENSION_NR),
        VFIO_TYPE1_IOMMU as usize,
        "VFIO_CHECK_EXTENSION(TYPE1_IOMMU)",
    )?;
    if ext == 0 {
        return Err(ax_err_type!(InvalidInput, "VFIO TYPE1 IOMMU not supported"));
    }

    let group_path = format!("/dev/vfio/{}", vfio.iommu_group);
    let group_fd = open_rdwr(&group_path)?;

    let mut group_status = VfioGroupStatus {
        argsz: core::mem::size_of::<VfioGroupStatus>() as u32,
        flags: 0,
    };
    let _ = ioctl_ret(
        group_fd,
        io(VFIO_TYPE, VFIO_GROUP_GET_STATUS_NR),
        (&mut group_status as *mut VfioGroupStatus) as usize,
        "VFIO_GROUP_GET_STATUS",
    )?;
    if (group_status.flags & VFIO_GROUP_FLAGS_VIABLE) == 0 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!("VFIO group {} is not viable", vfio.iommu_group)
        ));
    }

    let _ = ioctl_ret(
        group_fd,
        io(VFIO_TYPE, VFIO_GROUP_SET_CONTAINER_NR),
        (&container_fd as *const i32) as usize,
        "VFIO_GROUP_SET_CONTAINER",
    )?;
    let _ = ioctl_ret(
        container_fd,
        io(VFIO_TYPE, VFIO_SET_IOMMU_NR),
        VFIO_TYPE1_IOMMU as usize,
        "VFIO_SET_IOMMU(TYPE1)",
    )?;

    let bdf_c = CString::new(bdf_str.as_str()).map_err(|_| {
        ax_err_type!(
            InvalidInput,
            format_args!("Invalid BDF CString: {}", bdf_str)
        )
    })?;
    let device_fd = ioctl_ret(
        group_fd,
        io(VFIO_TYPE, VFIO_GROUP_GET_DEVICE_FD_NR),
        bdf_c.as_ptr() as usize,
        "VFIO_GROUP_GET_DEVICE_FD",
    )?;

    info!(
        "VFIO runtime opened: container_fd={} group_fd={} device_fd={} host={}",
        container_fd, group_fd, device_fd, bdf_str
    );
    Ok((container_fd, group_fd, device_fd))
}

fn map_guest_ram_dma(container_fd: i32, guest_memory: &[GuestRegionMmap]) -> AxResult<()> {
    for (idx, region) in guest_memory.iter().enumerate() {
        let iova = region.start_addr().raw_value();
        let size = region.len();
        let host_ptr = region
            .get_host_address(MemoryRegionAddress(0))
            .map_err(|e| {
                ax_err_type!(
                    InvalidInput,
                    format_args!("Failed to get host addr for region {}: {}", idx, e)
                )
            })?;

        let mut map = VfioIommuType1DmaMap {
            argsz: core::mem::size_of::<VfioIommuType1DmaMap>() as u32,
            flags: VFIO_DMA_MAP_FLAG_READ | VFIO_DMA_MAP_FLAG_WRITE,
            vaddr: host_ptr as usize as u64,
            iova,
            size,
        };

        info!("VFIO DMA map region{}: {:#x?}", idx, &map);

        let _ = ioctl_ret(
            container_fd,
            io(VFIO_TYPE, VFIO_IOMMU_MAP_DMA_NR),
            (&mut map as *mut VfioIommuType1DmaMap) as usize,
            "VFIO_IOMMU_MAP_DMA",
        )?;

        info!(
            "VFIO DMA map region{} iova=[{:#x}~{:#x}) vaddr={:#x} size={:#x}",
            idx,
            iova,
            iova + size,
            map.vaddr,
            size
        );
    }
    Ok(())
}

fn read_cfg_u16(device_fd: i32, cfg_base: u64, reg_off: u64) -> AxResult<u16> {
    let mut bytes = [0u8; 2];
    let off = cfg_base + reg_off;
    let read_ret = unsafe {
        libc::pread(
            device_fd,
            bytes.as_mut_ptr() as *mut libc::c_void,
            2,
            off as libc::off_t,
        )
    };
    if read_ret != 2 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO config pread u16 failed at off={:#x}: {}",
                off,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(u16::from_le_bytes(bytes))
}

fn read_cfg_u8(device_fd: i32, cfg_base: u64, reg_off: u64) -> AxResult<u8> {
    let mut byte = [0u8; 1];
    let off = cfg_base + reg_off;
    let read_ret = unsafe {
        libc::pread(
            device_fd,
            byte.as_mut_ptr() as *mut libc::c_void,
            1,
            off as libc::off_t,
        )
    };
    if read_ret != 1 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO config pread u8 failed at off={:#x}: {}",
                off,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(byte[0])
}

fn write_cfg_u16(device_fd: i32, cfg_base: u64, reg_off: u64, val: u16) -> AxResult<()> {
    let bytes = val.to_le_bytes();
    let off = cfg_base + reg_off;
    let write_ret = unsafe {
        libc::pwrite(
            device_fd,
            bytes.as_ptr() as *const libc::c_void,
            2,
            off as libc::off_t,
        )
    };
    if write_ret != 2 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO config pwrite u16 failed at off={:#x}: {}",
                off,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(())
}

fn read_region_u32(device_fd: i32, region_base: u64, reg_off: u64) -> AxResult<u32> {
    let mut bytes = [0u8; 4];
    let off = region_base + reg_off;
    let read_ret = unsafe {
        libc::pread(
            device_fd,
            bytes.as_mut_ptr() as *mut libc::c_void,
            4,
            off as libc::off_t,
        )
    };
    if read_ret != 4 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO region pread u32 failed at off={:#x}: {}",
                off,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(u32::from_le_bytes(bytes))
}

fn read_region_u32_be(device_fd: i32, region_base: u64, reg_off: u64) -> AxResult<u32> {
    let mut bytes = [0u8; 4];
    let off = region_base + reg_off;
    let read_ret = unsafe {
        libc::pread(
            device_fd,
            bytes.as_mut_ptr() as *mut libc::c_void,
            4,
            off as libc::off_t,
        )
    };
    if read_ret != 4 {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO region pread u32(be) failed at off={:#x}: {}",
                off,
                std::io::Error::last_os_error()
            )
        ));
    }
    Ok(u32::from_be_bytes(bytes))
}

fn read_region_u32_le(device_fd: i32, region_base: u64, reg_off: u64) -> AxResult<u32> {
    read_region_u32(device_fd, region_base, reg_off)
}

fn trace_mlx5_initseg_bar0(state: &VfioRuntimeState, reason: &str, force: bool) -> AxResult<()> {
    if state.bar0_region_size < (MLX5_INIT_SEG_CMD_DBELL_OFF + 4) {
        return Ok(());
    }
    let bar0 = state.bar0_region_offset;
    // mlx5 init-segment registers are big-endian in MMIO space.
    let cmdq_h = read_region_u32_be(state.device_fd, bar0, MLX5_INIT_SEG_CMDQ_ADDR_H_OFF)?;
    let cmdq_l_sz = read_region_u32_be(state.device_fd, bar0, MLX5_INIT_SEG_CMDQ_ADDR_L_SZ_OFF)?;
    let cmd_dbell = read_region_u32_be(state.device_fd, bar0, MLX5_INIT_SEG_CMD_DBELL_OFF)?;
    let nic_ifc = (cmdq_l_sz >> 8) & 0x7;

    let mut last = VFIO_MLX5_BAR0_LAST.lock().unwrap();
    let cur = (cmdq_h, cmdq_l_sz, cmd_dbell);
    let changed = last.map(|v| v != cur).unwrap_or(true);
    if force || changed {
        info!(
            "VFIO BAR0 initseg ({}) cmdq_h={:#010x} cmdq_l_sz={:#010x} nic_ifc={} cmd_dbell={:#010x}",
            reason, cmdq_h, cmdq_l_sz, nic_ifc, cmd_dbell
        );
    }
    *last = Some(cur);
    Ok(())
}

fn trace_mlx5_cmdq_activity(state: &VfioRuntimeState, reason: &str, force: bool) -> AxResult<()> {
    if state.bar0_region_size < (MLX5_INIT_SEG_CMDQ_ADDR_L_SZ_OFF + 4) {
        return Ok(());
    }
    let bar0 = state.bar0_region_offset;
    let cmdq_h = read_region_u32_be(state.device_fd, bar0, MLX5_INIT_SEG_CMDQ_ADDR_H_OFF)? as u64;
    let cmdq_l_sz = read_region_u32_be(state.device_fd, bar0, MLX5_INIT_SEG_CMDQ_ADDR_L_SZ_OFF)?;
    let cmdq_pa = (cmdq_h << 32) | ((cmdq_l_sz as u64) & 0xffff_f000);
    if cmdq_pa < state.guest_ram_iova_base
        || cmdq_pa
            >= state
                .guest_ram_iova_base
                .saturating_add(state.guest_ram_size)
    {
        return Ok(());
    }
    let off = (cmdq_pa - state.guest_ram_iova_base) as usize;
    let sample_len = 64usize;
    if off + sample_len > state.guest_ram_size as usize {
        return Ok(());
    }
    let ptr = (state.guest_ram_vaddr_base as usize + off) as *const u8;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, sample_len) };
    let mut sig: u64 = 0;
    for (i, b) in bytes.iter().enumerate() {
        sig ^= (*b as u64) << ((i % 8) * 8);
    }
    let head = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

    let mut last = VFIO_CMDQ_LAST_SIG.lock().unwrap();
    let cur = (sig, head);
    let changed = last.map(|v| v != cur).unwrap_or(true);
    if force || changed {
        info!(
            "VFIO cmdq sample ({}) pa={:#x} off={:#x} sig={:#x} head_dw0={:#010x}",
            reason, cmdq_pa, off, sig, head
        );
    }
    *last = Some(cur);
    Ok(())
}

fn cmdq_ram_trace_enabled() -> bool {
    // IMPORTANT:
    // The guest RAM mapping exposed through eqdriver/eqvisor is a temporary
    // bootstrap mapping for kernel/initrd loading. Hypercall HMicroVMBoot
    // unmaps it on the host VM side. Accessing guest RAM through the stale
    // host virtual pointer after boot can trigger host VM nested page faults.
    //
    // Keep this trace disabled by default. Only enable for short, controlled
    // experiments when the bootstrap mapping lifetime is guaranteed.
    std::env::var("AXCLI_VFIO_TRACE_CMDQ_RAM")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn active_route_fallback_enabled() -> bool {
    std::env::var("AXCLI_VFIO_DRAIN_ACTIVE_ROUTE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn find_pci_capability(device_fd: i32, cfg_base: u64, cap_id: u8) -> AxResult<Option<u64>> {
    // Some platforms can expose an inconsistent STATUS.CAP_LIST bit via VFIO
    // for VFs, but capability chain still exists. Do not hard-stop on CAP_LIST.
    let status = read_cfg_u16(device_fd, cfg_base, PCI_STATUS_REG_OFFSET).unwrap_or(0);
    let mut ptr = (read_cfg_u8(device_fd, cfg_base, PCI_CAP_PTR_OFFSET).unwrap_or(0) & 0xfc) as u64;
    let mut hops = 0usize;
    while ptr >= 0x40 && ptr < 0x100 && hops < 64 {
        let id = read_cfg_u8(device_fd, cfg_base, ptr)?;
        if id == cap_id {
            return Ok(Some(ptr));
        }
        let next = read_cfg_u8(device_fd, cfg_base, ptr + 1)?;
        if next == 0 {
            break;
        }
        ptr = (next & 0xfc) as u64;
        hops += 1;
    }
    // Fallback: brute-force scan to survive broken next pointers.
    for off in (0x40u64..0x100u64).step_by(4) {
        if read_cfg_u8(device_fd, cfg_base, off).unwrap_or(0xff) == cap_id {
            warn!(
                "VFIO capability fallback hit: cap_id={:#x} off={:#x} status={:#06x}",
                cap_id, off, status
            );
            return Ok(Some(off));
        }
    }
    Ok(None)
}

fn find_capability_in_snapshot(snapshot: &[u8], cfg_len: usize, cap_id: u8) -> Option<usize> {
    if cfg_len <= 0x40 {
        return None;
    }
    let mut cap_ptr = (snapshot[0x34] & 0xfc) as usize;
    let mut hops = 0usize;
    while cap_ptr >= 0x40 && cap_ptr + 1 < cfg_len && hops < 64 {
        if snapshot[cap_ptr] == cap_id {
            return Some(cap_ptr);
        }
        let next = (snapshot[cap_ptr + 1] & 0xfc) as usize;
        if next == 0 || next == cap_ptr {
            break;
        }
        cap_ptr = next;
        hops += 1;
    }
    None
}

fn read_snapshot_u16(snapshot: &[u8], off: usize) -> Option<u16> {
    if off + 2 > snapshot.len() {
        return None;
    }
    Some(u16::from_le_bytes([snapshot[off], snapshot[off + 1]]))
}

fn read_snapshot_u32(snapshot: &[u8], off: usize) -> Option<u32> {
    if off + 4 > snapshot.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        snapshot[off],
        snapshot[off + 1],
        snapshot[off + 2],
        snapshot[off + 3],
    ]))
}

fn sample_msix_table_vectors(state: &VfioRuntimeState, _force: bool) -> AxResult<()> {
    if state.msix_table_bir != Some(0) || state.msix_table_size == 0 {
        return Ok(());
    }
    if state.bar0_region_size < state.msix_table_offset {
        return Ok(());
    }

    let count = core::cmp::min(
        state.msix_table_size,
        core::cmp::min(state.msix_event_count, MAX_MSIX_EVENT_FDS),
    );
    let mut shadow = VFIO_MSIX_VECTOR_SHADOW.write().unwrap();
    let mut changed = 0usize;
    let mut unmasked_nonzero = 0usize;
    for idx in 0..count {
        let entry_off = state.msix_table_offset + (idx as u64) * PCI_MSIX_TABLE_ENTRY_SIZE;
        if entry_off + PCI_MSIX_TABLE_ENTRY_SIZE > state.bar0_region_size {
            break;
        }
        let msg_data =
            read_region_u32_le(state.device_fd, state.bar0_region_offset, entry_off + 8)?;
        let vector_ctrl =
            read_region_u32_le(state.device_fd, state.bar0_region_offset, entry_off + 12)?;
        let vector = (msg_data & 0xff) as u8;
        let masked = (vector_ctrl & 0x1) != 0;
        let new_vector = if !masked && vector != 0 {
            Some(vector)
        } else {
            None
        };
        if shadow[idx] != new_vector {
            info!(
                "VFIO MSI-X vector shadow entry={} msg_data={:#x} vector_ctrl={:#x} vector={:?}",
                idx, msg_data, vector_ctrl, new_vector
            );
            shadow[idx] = new_vector;
            changed += 1;
        }
        if new_vector.is_some() {
            unmasked_nonzero += 1;
        }
    }

    if _force || changed > 0 {
        info!(
            "VFIO MSI-X table sample: entries={} changed={} unmasked_nonzero_vectors={}",
            count, changed, unmasked_nonzero
        );
    }
    Ok(())
}

fn refresh_posted_irq_routes(state: &VfioRuntimeState) {
    if state.msix_event_count == 0 {
        return;
    }
    static LOGGED_DRAIN_MODE: std::sync::Once = std::sync::Once::new();
    LOGGED_DRAIN_MODE.call_once(|| {
        info!(
            "VFIO active posted-route eventfd drain mode: {}",
            active_route_fallback_enabled()
        );
    });
    let mut active = VFIO_MSIX_POSTED_ROUTE_ACTIVE.write().unwrap();
    for msix_index in 0..state.msix_event_count {
        match ioctl::ioctl_refresh_instance_irq_route(
            state.instance_fd,
            state.instance_id as u64,
            msix_index as u32,
        ) {
            Ok(true) => {
                if !active[msix_index] {
                    active[msix_index] = true;
                    info!(
                        "VFIO MSI-X route {} switched to posted-interrupt offload",
                        msix_index
                    );
                }
            }
            Ok(false) => {
                if active[msix_index] {
                    active[msix_index] = false;
                    info!(
                        "VFIO MSI-X route {} switched to software interrupt fallback",
                        msix_index
                    );
                } else {
                    trace!(
                        "VFIO MSI-X route {} posted-interrupt not ready yet",
                        msix_index
                    );
                }
            }
            Err(e) => {
                warn!(
                    "VFIO MSI-X route {} posted-interrupt refresh failed: {}",
                    msix_index, e
                );
            }
        }
    }
}

fn vfio_keep_device_ready(state: &VfioRuntimeState) -> AxResult<()> {
    let cfg_base = state.cfg_region_offset;
    let old_cmd = read_cfg_u16(state.device_fd, cfg_base, PCI_COMMAND_REG_OFFSET)?;
    let new_cmd = old_cmd | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER;
    if new_cmd != old_cmd {
        write_cfg_u16(state.device_fd, cfg_base, PCI_COMMAND_REG_OFFSET, new_cmd)?;
    }

    // Keep device in D0 so guest init sequence does not hang behind PM state.
    if let Some(pm_cap) = find_pci_capability(state.device_fd, cfg_base, PCI_CAP_ID_PM)? {
        let pmcsr_off = pm_cap + PCI_PMCSR_OFFSET_IN_CAP;
        let old_pmcsr = read_cfg_u16(state.device_fd, cfg_base, pmcsr_off)?;
        let new_pmcsr = old_pmcsr & !0x3;
        if new_pmcsr != old_pmcsr {
            write_cfg_u16(state.device_fd, cfg_base, pmcsr_off, new_pmcsr)?;
            /*
            info!(
                "VFIO PMCSR keepalive: cap={:#x} off={:#x} old={:#06x} new={:#06x} (force D0)",
                pm_cap,
                cfg_base + pmcsr_off,
                old_pmcsr,
                new_pmcsr
            );
            */
        }
    }

    // IRQ path must be enabled on the real host function too. Guest config
    // writes are currently shadowed in EqVisor, so we proactively keep real
    // MSI-X enabled/unmasked here. Otherwise eventfd never gets triggered.
    if let Some(msix_ctrl_off) = state.msix_ctrl_off {
        let old_msix = read_cfg_u16(state.device_fd, cfg_base, msix_ctrl_off)?;
        let new_msix = (old_msix | PCI_MSIX_FLAGS_ENABLE) & !PCI_MSIX_FLAGS_MASKALL;
        if new_msix != old_msix {
            write_cfg_u16(state.device_fd, cfg_base, msix_ctrl_off, new_msix)?;
            /*
            info!(
                "VFIO MSI-X keepalive: off={:#x} old={:#06x} new={:#06x} (enable, clear function-mask)",
                cfg_base + msix_ctrl_off,
                old_msix,
                new_msix
            );
            */
        }
    } else if VFIO_MSIX_CTRL_LOGGED.set(()).is_ok() {
        warn!("VFIO MSI-X keepalive: MSI-X capability offset is unavailable");
    }

    /*
    info!(
        "VFIO command keepalive: off={:#x} old={:#06x} new={:#06x} (MEM|BUSMASTER)",
        cfg_base + PCI_COMMAND_REG_OFFSET,
        old_cmd,
        new_cmd
    );
    */
    Ok(())
}

fn setup_vfio_msix_eventfds(
    instance_fd: i32,
    instance_id: usize,
    device_fd: i32,
) -> AxResult<(usize, [i32; MAX_MSIX_EVENT_FDS])> {
    let mut irq_info = VfioIrqInfo {
        argsz: core::mem::size_of::<VfioIrqInfo>() as u32,
        flags: 0,
        index: VFIO_PCI_MSIX_IRQ_INDEX,
        count: 0,
    };
    let _ = ioctl_ret(
        device_fd,
        io(VFIO_TYPE, VFIO_DEVICE_GET_IRQ_INFO_NR),
        (&mut irq_info as *mut VfioIrqInfo) as usize,
        "VFIO_DEVICE_GET_IRQ_INFO(MSIX)",
    )?;
    if irq_info.count == 0 {
        return Err(ax_err_type!(InvalidInput, "VFIO MSI-X irq count is zero"));
    }
    if irq_info.count as usize > MAX_MSIX_EVENT_FDS {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO MSI-X vectors {} exceed supported max {}",
                irq_info.count, MAX_MSIX_EVENT_FDS
            )
        ));
    }

    let mut fds = [-1i32; MAX_MSIX_EVENT_FDS];
    for i in 0..irq_info.count as usize {
        let event_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if event_fd < 0 {
            return Err(ax_err_type!(
                InvalidInput,
                format_args!(
                    "Failed to create eventfd for MSI-X vector {}: {}",
                    i,
                    std::io::Error::last_os_error()
                )
            ));
        }
        fds[i] = event_fd;
    }

    // VFIO_DEVICE_SET_IRQS uses a variable-length payload:
    // struct vfio_irq_set header + count * __s32 eventfd entries.
    let irq_set_hdr = VfioIrqSetHeader {
        argsz: (core::mem::size_of::<VfioIrqSetHeader>()
            + (irq_info.count as usize) * core::mem::size_of::<i32>()) as u32,
        flags: VFIO_IRQ_SET_ACTION_TRIGGER | VFIO_IRQ_SET_DATA_EVENTFD,
        index: VFIO_PCI_MSIX_IRQ_INDEX,
        start: 0,
        count: irq_info.count,
    };
    let mut irq_set_blob = vec![0u8; irq_set_hdr.argsz as usize];
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            (&irq_set_hdr as *const VfioIrqSetHeader).cast::<u8>(),
            core::mem::size_of::<VfioIrqSetHeader>(),
        )
    };
    irq_set_blob[..hdr_bytes.len()].copy_from_slice(hdr_bytes);
    let data_off = core::mem::size_of::<VfioIrqSetHeader>();
    for i in 0..irq_info.count as usize {
        let bytes = fds[i].to_ne_bytes();
        let off = data_off + i * core::mem::size_of::<i32>();
        irq_set_blob[off..off + core::mem::size_of::<i32>()].copy_from_slice(&bytes);
    }

    let _ = ioctl_ret(
        device_fd,
        io(VFIO_TYPE, VFIO_DEVICE_SET_IRQS_NR),
        irq_set_blob.as_mut_ptr() as usize,
        "VFIO_DEVICE_SET_IRQS(MSIX,eventfd-all)",
    )?;
    info!(
        "VFIO MSI-X eventfds armed: index={} start={} count={} total_vectors={}",
        VFIO_PCI_MSIX_IRQ_INDEX, irq_set_hdr.start, irq_set_hdr.count, irq_info.count
    );

    for i in 0..irq_info.count as usize {
        match ioctl::ioctl_register_instance_irq_route(
            instance_fd,
            instance_id as u64,
            fds[i],
            i as u32,
        ) {
            Ok(active) => {
                VFIO_MSIX_POSTED_ROUTE_ACTIVE.write().unwrap()[i] = active;
                if active {
                    info!(
                        "VFIO MSI-X route {} registered as posted-interrupt offload",
                        i
                    );
                } else {
                    info!(
                        "VFIO MSI-X route {} registered for irq_bypass, software forwarding fallback remains active",
                        i
                    );
                }
            }
            Err(e) => {
                warn!(
                    "VFIO MSI-X route {} irq_bypass registration failed: {}; software forwarding fallback remains active",
                    i, e
                );
            }
        }
    }

    Ok((irq_info.count as usize, fds))
}

fn drain_msix_event_and_forward(state: &VfioRuntimeState) {
    if state.msix_event_count == 0 {
        return;
    }
    for msix_index in 0..state.msix_event_count {
        let fd = state.msix_event_fds[msix_index];
        if fd < 0 {
            continue;
        }
        let posted_route_active = VFIO_MSIX_POSTED_ROUTE_ACTIVE.read().unwrap()[msix_index];
        if posted_route_active && !active_route_fallback_enabled() {
            continue;
        }
        loop {
            let mut cnt: u64 = 0;
            let n = unsafe {
                libc::read(
                    fd,
                    (&mut cnt as *mut u64).cast::<libc::c_void>(),
                    core::mem::size_of::<u64>(),
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::WouldBlock {
                    warn!(
                        "VFIO MSI-X eventfd read failed (idx={}): {}",
                        msix_index, err
                    );
                }
                break;
            }
            if n as usize != core::mem::size_of::<u64>() {
                break;
            }
            let inject_times = core::cmp::min(cnt as usize, 128);
            for _ in 0..inject_times {
                if let Err(e) = ioctl::ioctl_inject_instance_irq(
                    state.instance_fd,
                    state.instance_id as u64,
                    msix_index as u32,
                ) {
                    warn!(
                        "Forward VFIO MSI-X to EqVisor failed: instance={} msix_index={} err={}",
                        state.instance_id, msix_index, e
                    );
                    break;
                }
            }
            if posted_route_active
                && VFIO_ACTIVE_ROUTE_FALLBACK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed)
                    < VFIO_ACTIVE_ROUTE_FALLBACK_TRACE_LIMIT
            {
                info!(
                    "VFIO MSI-X software fallback forwarded while posted route is active: instance={} msix_index={} event_cnt={} injected={}",
                    state.instance_id, msix_index, cnt, inject_times
                );
            }
        }
    }
}

pub fn setup_vfio_dma_holder(
    resources: &VmResources,
    guest_memory: &[GuestRegionMmap],
) -> AxResult<()> {
    if resources.vfio.is_none() {
        return Ok(());
    }

    let (container_fd, group_fd, device_fd) = setup_vfio_fds(resources)?;
    map_guest_ram_dma(container_fd, guest_memory)?;
    let mut region = VfioRegionInfo {
        argsz: core::mem::size_of::<VfioRegionInfo>() as u32,
        flags: 0,
        index: VFIO_PCI_CONFIG_REGION_INDEX,
        cap_offset: 0,
        size: 0,
        offset: 0,
    };
    let _ = ioctl_ret(
        device_fd,
        io(VFIO_TYPE, VFIO_DEVICE_GET_REGION_INFO_NR),
        (&mut region as *mut VfioRegionInfo) as usize,
        "VFIO_DEVICE_GET_REGION_INFO(CONFIG)",
    )?;
    if region.size < (PCI_COMMAND_REG_OFFSET + 2) {
        return Err(ax_err_type!(
            InvalidInput,
            format_args!(
                "VFIO config region too small: size={:#x}, offset={:#x}",
                region.size, region.offset
            )
        ));
    }
    let mut state = VfioRuntimeState {
        container_fd,
        group_fd,
        device_fd,
        instance_fd: resources.fd,
        instance_id: resources.vm_id,
        msix_event_count: 0,
        msix_event_fds: [-1; MAX_MSIX_EVENT_FDS],
        msix_ctrl_off: None,
        msix_table_bir: None,
        msix_table_offset: 0,
        msix_table_size: 0,
        cfg_region_offset: region.offset,
        cfg_region_size: region.size,
        bar0_region_offset: 0,
        bar0_region_size: 0,
        guest_ram_iova_base: 0,
        guest_ram_vaddr_base: 0,
        guest_ram_size: 0,
    };
    if let Some(region0) = guest_memory.first() {
        let host_ptr = region0
            .get_host_address(MemoryRegionAddress(0))
            .map_err(|e| {
                ax_err_type!(
                    InvalidInput,
                    format_args!("Failed to get host addr for guest region0: {}", e)
                )
            })?;
        state.guest_ram_iova_base = region0.start_addr().raw_value();
        state.guest_ram_vaddr_base = host_ptr as usize as u64;
        state.guest_ram_size = region0.len();
    }
    let mut bar0_region = VfioRegionInfo {
        argsz: core::mem::size_of::<VfioRegionInfo>() as u32,
        flags: 0,
        index: VFIO_PCI_BAR0_REGION_INDEX,
        cap_offset: 0,
        size: 0,
        offset: 0,
    };
    if ioctl_ret(
        device_fd,
        io(VFIO_TYPE, VFIO_DEVICE_GET_REGION_INFO_NR),
        (&mut bar0_region as *mut VfioRegionInfo) as usize,
        "VFIO_DEVICE_GET_REGION_INFO(BAR0)",
    )
    .is_ok()
    {
        state.bar0_region_offset = bar0_region.offset;
        state.bar0_region_size = bar0_region.size;
        info!(
            "VFIO BAR0 region discovered: off={:#x} size={:#x}",
            state.bar0_region_offset, state.bar0_region_size
        );
    } else {
        warn!("VFIO BAR0 region info unavailable");
    }
    if let Some(vfio_cfg) = resources.vfio {
        let cfg_len = vfio_cfg.pci_cfg_space_len.min(vfio_cfg.pci_cfg_space.len());
        if let Some(msix_cap_off) =
            find_capability_in_snapshot(&vfio_cfg.pci_cfg_space, cfg_len, PCI_CAP_ID_MSIX)
        {
            state.msix_ctrl_off = Some((msix_cap_off as u64) + PCI_MSIX_FLAGS_OFFSET_IN_CAP);
            if let (Some(ctrl), Some(table)) = (
                read_snapshot_u16(&vfio_cfg.pci_cfg_space, msix_cap_off + 2),
                read_snapshot_u32(&vfio_cfg.pci_cfg_space, msix_cap_off + 4),
            ) {
                state.msix_table_size = ((ctrl & 0x07ff) as usize) + 1;
                state.msix_table_bir = Some(table & 0x7);
                state.msix_table_offset = (table & !0x7) as u64;
            }
            info!(
                "VFIO MSI-X metadata from snapshot: cap={:#x} ctrl_off={:#x} table_bir={:?} table_off={:#x} table_size={}",
                msix_cap_off,
                state.msix_ctrl_off.unwrap(),
                state.msix_table_bir,
                state.msix_table_offset,
                state.msix_table_size
            );
        } else {
            warn!("VFIO MSI-X capability not found in config snapshot");
        }
    }
    let (msix_event_count, msix_event_fds) =
        setup_vfio_msix_eventfds(resources.fd, resources.vm_id, device_fd)?;
    let state = VfioRuntimeState {
        msix_event_count,
        msix_event_fds,
        ..state
    };
    // Do not rely on host CF8/CFC path from EqVisor: keep critical command/PM
    // bits through VFIO PCI config region in host userspace backend.
    vfio_keep_device_ready(&state)?;
    let _ = sample_msix_table_vectors(&state, true);
    let _ = trace_mlx5_initseg_bar0(&state, "init", true);
    if cmdq_ram_trace_enabled() {
        let _ = trace_mlx5_cmdq_activity(&state, "init", true);
    } else if !VFIO_CMDQ_TRACE_WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            "VFIO cmdq RAM trace is disabled by default (set AXCLI_VFIO_TRACE_CMDQ_RAM=1 to enable, risky after HMicroVMBoot unmap)"
        );
    }
    let _ = VFIO_RUNTIME_STATE.set(state);
    info!(
        "VFIO DMA holder armed in current process (container_fd={} group_fd={} device_fd={})",
        container_fd, group_fd, device_fd
    );
    Ok(())
}

pub fn run_foreground_daemon_loop() -> ! {
    info!("axcli enters foreground daemon loop to keep VFIO/DMA alive");
    let mut last_keepalive = Instant::now() - Duration::from_secs(10);
    let mut last_bar0_trace = Instant::now() - Duration::from_secs(1);
    let mut last_cmdq_trace = Instant::now() - Duration::from_secs(1);
    let mut last_route_refresh = Instant::now() - Duration::from_secs(1);
    let route_refresh_start = Instant::now();
    loop {
        crate::microvm::console::poll_console_once();
        crate::microvm::control::poll_control_once();
        if let Some(state) = VFIO_RUNTIME_STATE.get() {
            drain_msix_event_and_forward(state);
            if last_bar0_trace.elapsed() >= Duration::from_millis(100) {
                if let Err(e) = sample_msix_table_vectors(state, false) {
                    warn!("VFIO MSI-X table sample failed: {:?}", e);
                }
                if let Err(e) = trace_mlx5_initseg_bar0(state, "poll", false) {
                    warn!("VFIO BAR0 trace failed: {:?}", e);
                }
                last_bar0_trace = Instant::now();
            }
            let route_refresh_interval = if route_refresh_start.elapsed() < Duration::from_secs(30)
            {
                Duration::from_millis(20)
            } else {
                Duration::from_millis(100)
            };
            if last_route_refresh.elapsed() >= route_refresh_interval {
                refresh_posted_irq_routes(state);
                last_route_refresh = Instant::now();
            }
            if cmdq_ram_trace_enabled() && last_cmdq_trace.elapsed() >= Duration::from_millis(100) {
                if let Err(e) = trace_mlx5_cmdq_activity(state, "poll", false) {
                    warn!("VFIO cmdq trace failed: {:?}", e);
                }
                last_cmdq_trace = Instant::now();
            }
            if last_keepalive.elapsed() >= Duration::from_secs(2) {
                if let Err(e) = vfio_keep_device_ready(state) {
                    warn!("VFIO keepalive failed: {:?}", e);
                }
                last_keepalive = Instant::now();
            }
            let _ = state.container_fd;
            let _ = state.group_fd;
            let _ = state.cfg_region_size;
        }
        thread::sleep(Duration::from_millis(20));
    }
}
