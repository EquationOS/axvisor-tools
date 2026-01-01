use std::ffi::CStr;

use libc::c_char;

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
    memory_size: usize,
) -> Result<usize, String> {
    let fd = open_eqmanager_dev()?;

    let mut arg = eq_create_instance_arg_t {
        instance_id: 0xdeadbeef,             // 0 means the kernel will assign an ID
        instance_type: 2,                    // 2 for microVM
        mapping_type: 0,                     // 0 for FlatMapping
        init_vcpu_num: init_vcpu_num as u64, // Initial vCPU number
        max_vcpu_num: max_vcpu_num as u64,
        memory_size: memory_size as u64,
    };

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

    Ok(arg.instance_id as usize)
}

pub fn ioctl_create_libos() -> Result<usize, String> {
    let fd = open_eqmanager_dev()?;

    let mut arg = eq_create_instance_arg_t {
        instance_id: 0xdeadbeef, // 0 means the kernel will assign an ID
        instance_type: 1,        // 1 for dynamic loading instance
        mapping_type: 1,         // 1 for CoarseGrainedSegmentation2M
        init_vcpu_num: 0,        // Dummy value, not used for libOS
        max_vcpu_num: 0,         // Dummy value, not used for libOS
        memory_size: 0,          // Dummy value, not used for libOS
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
