use axerrno::{LinuxError, LinuxResult};

use equation_defs::shm::ShmArgs;

use crate::hvc;
use crate::proxy::instance_id;

/// Proxy for the `shmget` syscall,
/// the daemon process does nothing but just forward the request to
/// the Linux kernel on behalf of the instance.
/// Returns the shared memory ID on success.
pub fn sys_shmget(key: u64, size: u64, shmflg: u64) -> LinuxResult<u64> {
    let res = unsafe {
        libc::shmget(
            key as libc::key_t,
            size as libc::size_t,
            shmflg as libc::c_int,
        )
    };

    if res < 0 {
        let error = std::io::Error::last_os_error();
        error!(
            "shmget failed with key: {:#x}, size: {:#x}, flags: {:#x}, errno: {}",
            key, size, shmflg, error
        );
        return Err(
            LinuxError::try_from(error.raw_os_error().unwrap_or(-1)).unwrap_or(LinuxError::EINVAL)
        );
    }

    debug!(
        "Proxying shmget key: {:#x}, size: {:#x}, flags: {:#x}, ret {}",
        key, size, shmflg, res as u64
    );

    Ok(res as u64)
}

/// Reads the value at `ptr` once using a volatile read.
#[inline(always)]
pub unsafe fn access_once<T>(ptr: *const T) -> T {
    core::ptr::read_volatile(ptr)
}

unsafe fn touch_mapping(addr: *mut libc::c_void, size: usize) {
    let ptr = addr as *const u8;
    // Access the memory region page by page to ensure it is mapped
    // and to avoid issues with lazy allocation.
    for i in (0..size).step_by(0x1000) {
        unsafe { access_once(ptr.add(i)) };
    }
}

/// Proxy for the `shmat` syscall with extra arguments from `shmget`,
/// the daemon process will forward the `shmat` request to the Linux kernel
/// on behalf of the instance.
/// Most importantly, it will also locked the actual shared memory region's physical pages,
/// and sync the mapping with the instance through hypercalls.
/// Returns the address of the shared memory on success.
pub fn sys_shmat_with_shmget_args(
    shmid: i32,
    shmaddr: u64,
    shmat_flg: i32,
    shmget_args: u64,
) -> LinuxResult<u64> {
    let res = unsafe {
        libc::shmat(
            shmid as libc::c_int,
            shmaddr as *const libc::c_void,
            shmat_flg as libc::c_int,
        )
    };

    let shmget_args = unsafe { (shmget_args as *mut ShmArgs).as_mut().unwrap() };
    let size = shmget_args.size;
    let shmkey = shmget_args.shmkey;
    let shmget_flg = shmget_args.shmflg;

    if res == libc::MAP_FAILED {
        error!(
            "shmat failed with shmid: {}, addr: {:#x}, flags: {:#x}, errno: {}",
            shmid,
            shmaddr,
            shmat_flg,
            std::io::Error::last_os_error()
        );
    } else {
        let shmaddr = res;

        debug!(
            "shmat succeeded with shmid: {}, addr: {:#p}, flags: {:#x}, size: {}",
            shmid, shmaddr, shmat_flg, size
        );

        unsafe {
            // Lock the shared memory region's physical pages.
            // Before locking, ensure the memory is mapped.
            touch_mapping(shmaddr, size as usize);
            // Lock the shared memory segment.
            // This is necessary to ensure the pages are not swapped out.
            let res = libc::shmctl(shmid, libc::SHM_LOCK, std::ptr::null_mut());
            if res < 0 {
                error!(
                    "Failed to lock shared memory segment with shmid: {}, errno: {}",
                    shmid,
                    std::io::Error::last_os_error()
                );
            }

            // Notify the hypervisor about the shared memory attachment.
            // This is necessary to sync the mapping with the instance.
            // The hypervisor will handle the actual mapping in the instance's address space.
            // The instance ID is used to identify the instance that is attaching the shared memory.
            // The hypervisor will return the GPA (guest physical address) of the shared memory region
            // which is then used by the instance to access the shared memory.
            let res = hvc::hvc_daemon_shmat(
                instance_id() as u64,
                shmkey as u64,
                shmaddr as u64,
                size as u64,
                shmget_flg as u64,
            );

            if res == -1 {
                error!(
                    "Failed to attach shared memory for instance ID: {}, errno: {}",
                    instance_id(),
                    res
                );
                return Err(LinuxError::ENOMEM);
            }

            trace!("Get instance shm_gpa {:#x}", res as usize);

            // Update the shmget_args with the actual shared memory address.
            shmget_args.shmgva = shmaddr as usize;
            // The shared memory GPA (guest physical address) is set to the result of the hypercall.
            // This is the address that the instance will use to access the shared memory.
            // It is assumed that the hypercall will return the GPA of the shared memory region.
            shmget_args.shmgpa = res as usize;
        }
    }

    debug!(
        "Proxying shmat shmid: {}, addr: {:#x}, flags: {:#x}, key: {:#x}, size: {:#x}, addr {:#x}",
        shmid, shmaddr, shmat_flg, shmkey, size, res as u64
    );

    Ok(res as u64)
}

pub fn sys_shmdt(shmaddr: u64) -> LinuxResult<u64> {
    let res = unsafe { libc::shmdt(shmaddr as *const libc::c_void) };

    debug!("Proxying shmdt addr: {:#x}, ret {}", shmaddr, res);

    if res < 0 {
        error!(
            "shmdt failed with addr: {:#x}, errno: {}",
            shmaddr,
            std::io::Error::last_os_error()
        );
    }

    Ok(res as u64)
}
