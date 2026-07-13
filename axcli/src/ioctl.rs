use std::ffi::CStr;

use crate::microvm::{IovaMode, PciBdf, VfioResourceConfig};
#[cfg(feature = "microvm")]
use eqvm_defs::{
    EqHyperAllocDebugReclaimReq, EqHyperAllocEqGateDebugEnqueueReq, EqHyperAllocEqGateDrainReq,
    EqHyperAllocQuery, EqHyperAllocVfioDmaOp, EqMicroVmGuestMemCopy, EqMicroVmGuestRamMmapZap,
    EqMicroVmGuestRamMmapZapOp,
};
use libc::c_char;

pub const EQINSTANCE_DEV_PREFIX: &str = "/dev/eqinstance_";

#[derive(Debug, Clone, Copy)]
pub struct MicroVmCreateResult {
    pub instance_id: usize,
    pub console_ring_gpa: usize,
    pub block_notify_ring_gpa: usize,
}

#[cfg(feature = "microvm")]
#[derive(Debug, Clone, Copy)]
pub struct MicroVmGuestRamMmapState {
    pub generation: u64,
    pub active_mmaps: u64,
    pub current_mmaps: u64,
    pub stale_mmaps: u64,
}

include!(concat!(env!("OUT_DIR"), "/eqioctl.rs"));

/// Direction flags
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

/// Bit shifts
const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_DIRBITS: u32 = 2;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

/// Encode ioctl command (equivalent to _IOC macro)
const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> u64 {
    ((dir as u64) << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

/// _IOW type: write to kernel from user
const fn iow<T>(ty: u32, nr: u32) -> u64 {
    ioc(IOC_WRITE, ty, nr, size_of::<T>())
}

const EQ_CREATE_INSTANCE: u64 = iow::<eq_create_instance_arg_t>(0, 0);
const EQ_REMOVE_INSTANCE: u64 = iow::<eq_remove_instance_arg_t>(0, 1);
const EQ_INSTANCE_INJECT_IRQ: u64 = iow::<eq_instance_irq_inject_arg_t>(0, 2);
const EQ_INSTANCE_REGISTER_IRQ_ROUTE: u64 = iow::<eq_instance_irq_route_arg_t>(0, 3);
const EQ_INSTANCE_REFRESH_IRQ_ROUTE: u64 = iow::<eq_instance_irq_route_arg_t>(0, 4);
const EQ_INSTANCE_SET_VCPU_COUNT: u64 = iow::<eq_instance_vcpu_resize_arg_t>(0, 5);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL: u64 = iow::<EqHyperAllocVfioDmaOp>(0, 6);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_VFIO_DMA_COMPLETE: u64 = iow::<EqHyperAllocVfioDmaOp>(0, 7);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_GUEST_MEM_COPY: u64 = iow::<EqMicroVmGuestMemCopy>(0, 8);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY: u64 =
    iow::<eq_microvm_guest_ram_mmap_query_t>(0, 9);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_MEMORY_TARGET: u64 =
    iow::<eq_hyperalloc_pagecache_shrink_req_t>(0, 10);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_QUERY: u64 = iow::<EqHyperAllocQuery>(0, 11);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_VFIO_DMA_DEBUG_REQUEST: u64 = iow::<EqHyperAllocVfioDmaOp>(0, 12);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_EQGATE_DRAIN: u64 = iow::<EqHyperAllocEqGateDrainReq>(0, 13);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_EQGATE_DEBUG_ENQUEUE: u64 =
    iow::<EqHyperAllocEqGateDebugEnqueueReq>(0, 14);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_HYPERALLOC_DEBUG_RECLAIM: u64 = iow::<EqHyperAllocDebugReclaimReq>(0, 15);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP: u64 = iow::<EqMicroVmGuestRamMmapZap>(0, 16);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_POLL: u64 = iow::<EqMicroVmGuestRamMmapZapOp>(0, 17);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_COMPLETE: u64 =
    iow::<EqMicroVmGuestRamMmapZapOp>(0, 18);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_STOP: u64 = iow::<eq_microvm_stop_arg_t>(0, 19);
#[cfg(feature = "microvm")]
const EQ_INSTANCE_MICROVM_BOOT: u64 = iow::<eq_microvm_boot_arg_t>(0, 20);

const EQ_DEVICE_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"/dev/eqmanager\0") };

fn open_eqmanager_dev() -> Result<libc::c_int, String> {
    let fd = unsafe { libc::open(EQ_DEVICE_NAME.as_ptr() as *const c_char, libc::O_RDWR) };

    if fd < 0 {
        return Err(format!(
            "Failed to open {}, error {}",
            EQ_DEVICE_NAME.to_string_lossy(),
            std::io::Error::last_os_error()
        ));
    }

    Ok(fd)
}

pub fn ioctl_create_microvm(
    init_vcpu_num: u8,
    max_vcpu_num: u8,
    init_mem_size_mib: usize,
    max_mem_size_mib: usize,
    hyperalloc_retain_hpa: bool,
    passthrough_devices: &[PciBdf],
    vfio: Option<VfioResourceConfig>,
    block_device_count: usize,
    block_flags: u64,
    block_capacity_sectors: u64,
) -> Result<MicroVmCreateResult, String> {
    let fd = open_eqmanager_dev()?;

    let mut arg = eq_create_instance_arg_t {
        instance_id: 0xdeadbeef,             // 0 means the kernel will assign an ID
        instance_type: 2,                    // 2 for microVM
        mapping_type: 0,                     // 0 for FlatMapping
        init_vcpu_num: init_vcpu_num as u64, // Initial vCPU number
        max_vcpu_num: max_vcpu_num as u64,
        init_mem_size_mib: init_mem_size_mib as u64,
        max_mem_size_mib: max_mem_size_mib as u64,
        microvm_hyperalloc_flags: 0,
        passthrough_device_count: 0,
        passthrough_bdf: [0; 8],
        vfio_flags: 0,
        vfio_iommu_group: 0,
        vfio_guest_visible_bdf: 0,
        vfio_bar_count: 0,
        vfio_bar_start: [0; 6],
        vfio_bar_size: [0; 6],
        vfio_bar_flags: [0; 6],
        vfio_pci_cfg_space_len: 0,
        vfio_pci_cfg_space: [0; 256],
        microvm_console_ring_gpa: 0,
        microvm_block_flags: 0,
        microvm_block_device_count: 0,
        microvm_block_notify_ring_gpa: 0,
        microvm_block_capacity_sectors: 0,
    };

    if hyperalloc_retain_hpa {
        const MICROVM_HYPERALLOC_FLAG_RETAIN_HPA: u64 = 1 << 0;
        arg.microvm_hyperalloc_flags |= MICROVM_HYPERALLOC_FLAG_RETAIN_HPA;
    }

    if passthrough_devices.len() > arg.passthrough_bdf.len() {
        return Err(format!(
            "Too many passthrough devices: {}, max {}",
            passthrough_devices.len(),
            arg.passthrough_bdf.len()
        ));
    }
    arg.passthrough_device_count = passthrough_devices.len() as u64;
    for (i, bdf) in passthrough_devices.iter().enumerate() {
        arg.passthrough_bdf[i] = bdf.encode_u64();
    }
    if let Some(vfio_cfg) = vfio {
        const VFIO_FLAG_ENABLED: u64 = 1 << 0;
        const VFIO_FLAG_IOVA_GPA_IDENTITY: u64 = 1 << 1;
        const VFIO_FLAG_PHYSICAL_RELEASE: u64 = 1 << 2;
        arg.vfio_flags |= VFIO_FLAG_ENABLED;
        if vfio_cfg.iova_mode == IovaMode::GpaIdentity {
            arg.vfio_flags |= VFIO_FLAG_IOVA_GPA_IDENTITY;
        }
        if vfio_cfg.physical_release {
            arg.vfio_flags |= VFIO_FLAG_PHYSICAL_RELEASE;
        }
        arg.vfio_iommu_group = vfio_cfg.iommu_group as u64;
        arg.vfio_guest_visible_bdf = vfio_cfg.guest_visible_bdf.encode_u64();
        arg.vfio_bar_count = vfio_cfg.bars.len() as u64;
        for (i, bar) in vfio_cfg.bars.iter().enumerate() {
            arg.vfio_bar_start[i] = bar.start;
            arg.vfio_bar_size[i] = bar.size;
            arg.vfio_bar_flags[i] = bar.flags;
        }
        let cfg_len = core::cmp::min(vfio_cfg.pci_cfg_space_len, arg.vfio_pci_cfg_space.len());
        arg.vfio_pci_cfg_space_len = cfg_len as u64;
        arg.vfio_pci_cfg_space[..cfg_len].copy_from_slice(&vfio_cfg.pci_cfg_space[..cfg_len]);
    }
    if block_device_count > 0 {
        arg.microvm_block_flags |= block_flags;
        arg.microvm_block_device_count = block_device_count as u64;
        arg.microvm_block_capacity_sectors = block_capacity_sectors;
    }

    let ret = unsafe { libc::ioctl(fd, EQ_CREATE_INSTANCE as libc::c_ulong, &mut arg as *mut _) };

    if ret < 0 {
        return Err(format!(
            "Failed to create instance: {}",
            std::io::Error::last_os_error()
        ));
    }

    if arg.instance_id == 0xdeadbeef || arg.instance_id == 0 {
        return Err("Instance ID was not assigned by the kernel".to_string());
    }

    info!("Instance created successfully, ID: {}", arg.instance_id);

    Ok(MicroVmCreateResult {
        instance_id: arg.instance_id as usize,
        console_ring_gpa: arg.microvm_console_ring_gpa as usize,
        block_notify_ring_gpa: arg.microvm_block_notify_ring_gpa as usize,
    })
}

pub fn ioctl_create_libos() -> Result<usize, String> {
    let fd = open_eqmanager_dev()?;

    let mut arg = eq_create_instance_arg_t {
        instance_id: 0xdeadbeef, // 0 means the kernel will assign an ID
        instance_type: 1,        // 1 for dynamic loading instance
        mapping_type: 1,         // 1 for CoarseGrainedSegmentation2M
        init_vcpu_num: 0,        // Dummy value, not used for libOS
        max_vcpu_num: 0,         // Dummy value, not used for libOS
        init_mem_size_mib: 0,    // Dummy value, not used for libOS
        max_mem_size_mib: 0,     // Dummy value, not used for libOS
        microvm_hyperalloc_flags: 0,
        passthrough_device_count: 0,
        passthrough_bdf: [0; 8],
        vfio_flags: 0,
        vfio_iommu_group: 0,
        vfio_guest_visible_bdf: 0,
        vfio_bar_count: 0,
        vfio_bar_start: [0; 6],
        vfio_bar_size: [0; 6],
        vfio_bar_flags: [0; 6],
        vfio_pci_cfg_space_len: 0,
        vfio_pci_cfg_space: [0; 256],
        microvm_console_ring_gpa: 0,
        microvm_block_flags: 0,
        microvm_block_device_count: 0,
        microvm_block_notify_ring_gpa: 0,
        microvm_block_capacity_sectors: 0,
    };

    let ret = unsafe { libc::ioctl(fd, EQ_CREATE_INSTANCE as libc::c_ulong, &mut arg as *mut _) };

    if ret < 0 {
        return Err(format!(
            "Failed to create instance: {}",
            std::io::Error::last_os_error()
        ));
    }

    if arg.instance_id == 0xdeadbeef {
        return Err("Instance ID was not assigned by the kernel".to_string());
    }

    info!("Instance created successfully, ID: {}", arg.instance_id);

    Ok(arg.instance_id as usize)
}

pub fn ioctl_remove_instance(instance_id: u64) -> Result<(), String> {
    let fd = unsafe { libc::open(EQ_DEVICE_NAME.as_ptr() as *const c_char, libc::O_RDWR) };

    if fd < 0 {
        return Err(format!(
            "Failed to open {}, error {}",
            EQ_DEVICE_NAME.to_string_lossy(),
            std::io::Error::last_os_error()
        ));
    }

    let mut arg = eq_remove_instance_arg_t { instance_id };

    let ret = unsafe { libc::ioctl(fd, EQ_REMOVE_INSTANCE as libc::c_ulong, &mut arg as *mut _) };

    if ret < 0 {
        return Err(format!(
            "Failed to remove instance {}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }

    info!("Instance {} removed successfully", instance_id);
    Ok(())
}

pub fn ioctl_inject_instance_irq(
    instance_fd: i32,
    instance_id: u64,
    msix_index: u32,
) -> Result<(), String> {
    let mut arg = eq_instance_irq_inject_arg_t {
        instance_id,
        msix_index,
        reserved: 0,
    };
    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_INJECT_IRQ as libc::c_ulong,
            &mut arg as *mut _,
        )
    };
    if ret < 0 {
        return Err(format!(
            "Failed to inject instance irq (instance={} msix_index={}): {}",
            instance_id,
            msix_index,
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn ioctl_register_instance_irq_route(
    instance_fd: i32,
    instance_id: u64,
    eventfd: i32,
    msix_index: u32,
) -> Result<bool, String> {
    let mut arg = eq_instance_irq_route_arg_t {
        instance_id,
        eventfd,
        msix_index,
        flags: 0,
        reserved: 0,
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_REGISTER_IRQ_ROUTE as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to register IRQ route for instance {} msix_index {}: {}",
            instance_id,
            msix_index,
            std::io::Error::last_os_error()
        ));
    }

    Ok((arg.flags & 0x1) != 0)
}

pub fn ioctl_refresh_instance_irq_route(
    instance_fd: i32,
    instance_id: u64,
    msix_index: u32,
) -> Result<bool, String> {
    let mut arg = eq_instance_irq_route_arg_t {
        instance_id,
        eventfd: -1,
        msix_index,
        flags: 0,
        reserved: 0,
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_REFRESH_IRQ_ROUTE as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to refresh IRQ route for instance {} msix_index {}: {}",
            instance_id,
            msix_index,
            std::io::Error::last_os_error()
        ));
    }

    Ok((arg.flags & 0x1) != 0)
}

pub fn ioctl_set_instance_vcpu_count(
    instance_fd: i32,
    instance_id: u64,
    vcpu_count: u32,
) -> Result<(), String> {
    let mut arg = eq_instance_vcpu_resize_arg_t {
        instance_id,
        vcpu_count,
        flags: 0,
        reserved: [0; 2],
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_SET_VCPU_COUNT as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to set vCPU count for instance {} to {}: {}",
            instance_id,
            vcpu_count,
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_stop(instance_fd: i32, instance_id: u64) -> Result<u64, String> {
    let mut arg = eq_microvm_stop_arg_t {
        instance_id,
        active_pcpus_signalled: 0,
        flags: 0,
        reserved: [0; 2],
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_STOP as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to stop microVM instance {}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(arg.active_pcpus_signalled)
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_boot(
    instance_fd: i32,
    instance_id: u64,
    entry_point: u64,
    boot_protocol: u32,
) -> Result<(), String> {
    let mut arg = eq_microvm_boot_arg_t {
        instance_id,
        entry_point,
        boot_protocol,
        flags: 0,
        reserved: [0; 2],
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_BOOT as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to boot microVM instance {} entry_point={:#x} boot_protocol={}: {}",
            instance_id,
            entry_point,
            boot_protocol,
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_vfio_dma_poll(
    instance_fd: i32,
    instance_id: u64,
) -> Result<EqHyperAllocVfioDmaOp, String> {
    let mut arg = EqHyperAllocVfioDmaOp {
        version: eqvm_defs::EQ_HYPERALLOC_VERSION,
        instance_id,
        ..EqHyperAllocVfioDmaOp::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to poll HyperAlloc VFIO DMA op for instance {}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_vfio_dma_complete(
    instance_fd: i32,
    op: &mut EqHyperAllocVfioDmaOp,
) -> Result<(), String> {
    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_VFIO_DMA_COMPLETE as libc::c_ulong,
            op as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to complete HyperAlloc VFIO DMA op seq={} instance={}: {}",
            op.sequence,
            op.instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_vfio_dma_debug_request(
    instance_fd: i32,
    op: &mut EqHyperAllocVfioDmaOp,
) -> Result<(), String> {
    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_VFIO_DMA_DEBUG_REQUEST as libc::c_ulong,
            op as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to enqueue HyperAlloc VFIO DMA debug request instance={} op={} iova={:#x} size={:#x}: {}",
            op.instance_id,
            op.op,
            op.iova,
            op.size,
            std::io::Error::last_os_error()
        ));
    }
    if op.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected HyperAlloc VFIO DMA debug request instance={} op={} status={} errno={} iova={:#x} size={:#x}",
            op.instance_id, op.op, op.status, op.result_errno, op.iova, op.size
        ));
    }

    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_memory_target(
    instance_fd: i32,
    instance_id: u64,
    target_huge_frames: u64,
    timeout_ms: u64,
) -> Result<eq_hyperalloc_pagecache_shrink_req_t, String> {
    let mut arg = eq_hyperalloc_pagecache_shrink_req_t {
        version: EQ_HYPERALLOC_VERSION,
        flags: 0,
        sequence: 0,
        target_huge_frames,
        target_pages: 0,
        timeout_ms,
        reclaimed_huge_frames: 0,
        remaining_file_huge_frames: 0,
        status: EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_NONE,
        result_errno: 0,
        reserved: [0; 2],
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_MEMORY_TARGET as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to set HyperAlloc memory target for instance {} target_huge_frames={}: {}",
            instance_id,
            target_huge_frames,
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected HyperAlloc memory target for instance {} target_huge_frames={} errno={}",
            instance_id, target_huge_frames, arg.result_errno
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_query(
    instance_fd: i32,
    instance_id: u64,
) -> Result<EqHyperAllocQuery, String> {
    let mut arg = EqHyperAllocQuery::default();

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_QUERY as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to query HyperAlloc state for instance {}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }
    if arg.version != eqvm_defs::EQ_HYPERALLOC_VERSION {
        return Err(format!(
            "Invalid HyperAlloc query version for instance {}: got={} expected={}",
            instance_id,
            arg.version,
            eqvm_defs::EQ_HYPERALLOC_VERSION
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_debug_reclaim(
    instance_fd: i32,
    instance_id: u64,
    zone_id: u32,
    frame_gpa: u64,
    frame_len: u64,
    flags: u32,
) -> Result<EqHyperAllocDebugReclaimReq, String> {
    let mut arg = EqHyperAllocDebugReclaimReq {
        version: eqvm_defs::EQ_HYPERALLOC_DEBUG_RECLAIM_VERSION,
        flags,
        instance_id,
        zone_id,
        frame_gpa,
        frame_len,
        ..EqHyperAllocDebugReclaimReq::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_DEBUG_RECLAIM as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to debug reclaim HyperAlloc frame for instance {} zone={} gpa={:#x}: {}",
            instance_id,
            zone_id,
            frame_gpa,
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected HyperAlloc debug reclaim for instance {} zone={} gpa={:#x} errno={}",
            instance_id, zone_id, frame_gpa, arg.result_errno
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_ram_mmap_zap(
    instance_fd: i32,
    instance_id: u64,
    gpa: u64,
    len: u64,
) -> Result<EqMicroVmGuestRamMmapZap, String> {
    let mut arg = EqMicroVmGuestRamMmapZap {
        version: eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_VERSION,
        instance_id,
        gpa,
        len,
        ..EqMicroVmGuestRamMmapZap::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to zap MicroVM guest RAM mmap for instance {} gpa={:#x} len={:#x}: {}",
            instance_id,
            gpa,
            len,
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "Eqdriver rejected MicroVM guest RAM mmap zap for instance {} gpa={:#x} len={:#x} errno={}",
            instance_id, gpa, len, arg.result_errno
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_ram_mmap_zap_poll(
    instance_fd: i32,
    instance_id: u64,
) -> Result<EqMicroVmGuestRamMmapZapOp, String> {
    let mut arg = EqMicroVmGuestRamMmapZapOp {
        version: eqvm_defs::EQ_MICROVM_GUEST_RAM_MMAP_ZAP_OP_VERSION,
        instance_id,
        ..EqMicroVmGuestRamMmapZapOp::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_POLL as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to poll MicroVM guest RAM mmap zap op for instance {}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_ram_mmap_zap_complete(
    instance_fd: i32,
    op: &mut EqMicroVmGuestRamMmapZapOp,
) -> Result<(), String> {
    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_COMPLETE as libc::c_ulong,
            op as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to complete MicroVM guest RAM mmap zap op seq={} instance={}: {}",
            op.sequence,
            op.instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_eqgate_drain(
    instance_fd: i32,
    instance_id: u64,
    max_requests: u32,
    flags: u32,
) -> Result<EqHyperAllocEqGateDrainReq, String> {
    let mut arg = EqHyperAllocEqGateDrainReq {
        version: eqvm_defs::EQ_HYPERALLOC_EQGATE_DRAIN_VERSION,
        flags,
        instance_id,
        max_requests,
        ..EqHyperAllocEqGateDrainReq::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_EQGATE_DRAIN as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to drain HyperAlloc EqGate batch for instance {} max_requests={}: {}",
            instance_id,
            max_requests,
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected HyperAlloc EqGate drain for instance {} max_requests={} errno={}",
            instance_id, max_requests, arg.result_errno
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_hyperalloc_eqgate_debug_enqueue(
    instance_fd: i32,
    instance_id: u64,
    vcpu_id: u32,
    zone_id: u32,
    frame_gpa: u64,
    frame_len: u64,
    entry_flags: u64,
) -> Result<EqHyperAllocEqGateDebugEnqueueReq, String> {
    let mut arg = EqHyperAllocEqGateDebugEnqueueReq {
        version: eqvm_defs::EQ_HYPERALLOC_EQGATE_DEBUG_ENQUEUE_VERSION,
        instance_id,
        vcpu_id,
        zone_id,
        frame_gpa,
        frame_len,
        entry_flags,
        ..EqHyperAllocEqGateDebugEnqueueReq::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_HYPERALLOC_EQGATE_DEBUG_ENQUEUE as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to debug-enqueue HyperAlloc EqGate batch for instance {} vcpu={} zone={} gpa={:#x} len={:#x}: {}",
            instance_id,
            vcpu_id,
            zone_id,
            frame_gpa,
            frame_len,
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected HyperAlloc EqGate debug enqueue for instance {} vcpu={} zone={} gpa={:#x} len={:#x} errno={}",
            instance_id, vcpu_id, zone_id, frame_gpa, frame_len, arg.result_errno
        ));
    }

    Ok(arg)
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_mem_read(
    instance_fd: i32,
    instance_id: u64,
    gpa: u64,
    buf: &mut [u8],
) -> Result<(), String> {
    let mut arg = EqMicroVmGuestMemCopy {
        version: EQ_MICROVM_GUEST_MEM_COPY_VERSION,
        flags: EQ_MICROVM_GUEST_MEM_COPY_READ_FROM_GUEST,
        instance_id,
        gpa,
        len: buf.len() as u32,
        user_ptr: buf.as_mut_ptr() as u64,
        ..EqMicroVmGuestMemCopy::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_MEM_COPY as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to read MicroVM guest memory instance={} gpa={:#x} len={}: {}",
            instance_id,
            gpa,
            buf.len(),
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected MicroVM guest memory read instance={} gpa={:#x} len={} errno={}",
            instance_id,
            gpa,
            buf.len(),
            arg.result_errno
        ));
    }
    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_mem_write(
    instance_fd: i32,
    instance_id: u64,
    gpa: u64,
    buf: &[u8],
) -> Result<(), String> {
    let mut arg = EqMicroVmGuestMemCopy {
        version: EQ_MICROVM_GUEST_MEM_COPY_VERSION,
        flags: EQ_MICROVM_GUEST_MEM_COPY_WRITE_TO_GUEST,
        instance_id,
        gpa,
        len: buf.len() as u32,
        user_ptr: buf.as_ptr() as u64,
        ..EqMicroVmGuestMemCopy::default()
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_MEM_COPY as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to write MicroVM guest memory instance={} gpa={:#x} len={}: {}",
            instance_id,
            gpa,
            buf.len(),
            std::io::Error::last_os_error()
        ));
    }
    if arg.result_errno != 0 {
        return Err(format!(
            "EqVisor rejected MicroVM guest memory write instance={} gpa={:#x} len={} errno={}",
            instance_id,
            gpa,
            buf.len(),
            arg.result_errno
        ));
    }
    Ok(())
}

#[cfg(feature = "microvm")]
pub fn ioctl_microvm_guest_ram_mmap_state(
    instance_fd: i32,
    instance_id: u64,
) -> Result<MicroVmGuestRamMmapState, String> {
    let mut arg = eq_microvm_guest_ram_mmap_query_t {
        version: EQ_MICROVM_GUEST_RAM_MMAP_QUERY_VERSION,
        flags: 0,
        instance_id,
        generation: 0,
        active_mmaps: 0,
        current_mmaps: 0,
        stale_mmaps: 0,
        reserved: [0; 3],
    };

    let ret = unsafe {
        libc::ioctl(
            instance_fd,
            EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY as libc::c_ulong,
            &mut arg as *mut _,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to query MicroVM guest RAM mmap state instance={}: {}",
            instance_id,
            std::io::Error::last_os_error()
        ));
    }

    Ok(MicroVmGuestRamMmapState {
        generation: arg.generation,
        active_mmaps: arg.active_mmaps,
        current_mmaps: arg.current_mmaps,
        stale_mmaps: arg.stale_mmaps,
    })
}
