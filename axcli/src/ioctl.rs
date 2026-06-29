use std::ffi::CStr;

use crate::microvm::{IovaMode, PciBdf, VfioResourceConfig};
use libc::c_char;

pub const EQINSTANCE_DEV_PREFIX: &str = "/dev/eqinstance_";

#[derive(Debug, Clone, Copy)]
pub struct MicroVmCreateResult {
    pub instance_id: usize,
    pub console_ring_gpa: usize,
    pub block_notify_ring_gpa: usize,
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
        arg.vfio_flags |= VFIO_FLAG_ENABLED;
        if vfio_cfg.iova_mode == IovaMode::GpaIdentity {
            arg.vfio_flags |= VFIO_FLAG_IOVA_GPA_IDENTITY;
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
