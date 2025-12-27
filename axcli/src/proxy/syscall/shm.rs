use libc::MAP_LOCKED;

use axerrno::{LinuxError, LinuxResult};
use equation_defs::shm::ShmArgs;
use memory_addr::is_aligned_4k;
use memory_addr::PAGE_SIZE_2M;

use crate::hvc;
use crate::proxy::instance_id;
use crate::proxy::FD_LIST;

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
    // let shmget_flg = shmget_args.shmflg;

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

            // After pages are touched, read pagemap and print info for debugging.
            if false {
                print_pagemap_info(shmaddr as usize, size as usize);
            }

            // Notify the hypervisor about the shared memory attachment.
            // This is necessary to sync the mapping with the instance.
            // The hypervisor will handle the actual mapping in the instance's address space.
            // The instance ID is used to identify the instance that is attaching the shared memory,
            // The process ID is passed by SCF and used to identify the process within the instance,
            // Note that underlying hypervisor will establish the mapping based on the process ID
            // in the process's state-1 page table.
            let res = hvc::hvc_daemon_shmat(
                instance_id() as u64,
                shmget_args.process_id as u64,
                shmkey as u64,
                shmaddr as u64,
                size as u64,
                shmat_flg as u64,
            );

            if res < 0 {
                error!(
                    "Failed to attach shared memory for instance ID: {}, process {} errno: {}",
                    instance_id(),
                    shmget_args.process_id,
                    res
                );
                return Err(LinuxError::ENOMEM);
            }

            trace!("Get instance shm_gpa {:#x}", res as usize);

            // Update the shmget_args with the actual shared memory address.
            shmget_args.shmgva = shmaddr as usize;
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

/// Proxy for the `memfd_create` syscall,
/// the daemon process will create a memfd on behalf of the instance,
/// and keep track of the memfd's name for debugging purposes.
///
/// Note that it will not trigger a hypercall during `create_memfd`,
/// because the actual mapping will be handled during `mmap` to the memfd.
///
/// The `creae_memfd` is paired with `mmap` to this memfd,
/// just like how `shmget` is paired with `shmat`.
///  
/// Returns the memfd file descriptor on success.
pub fn sys_create_memfd(name: u64, flags: u64) -> LinuxResult<u64> {
    let name_ptr = name as *const i8;

    // Convert the pathname pointer to a Rust string
    let name = unsafe {
        let cstr = std::ffi::CStr::from_ptr(name_ptr);
        cstr.to_string_lossy().into_owned()
    };

    let memfd = unsafe { libc::memfd_create(name_ptr, flags as libc::c_uint) };

    if memfd > 0 {
        info!("Created memfd {} with fd: {}", name, memfd);
        FD_LIST
            .lock()
            .unwrap()
            .insert(memfd, format!("memfd_{}", name));
    } else {
        error!(
            "Failed to create memfd {}, error: {}",
            name,
            std::io::Error::last_os_error()
        );
    }

    debug!(
        "Proxying memfd_create name: {}, flags: {:#x}, memfd {}",
        name, flags, memfd
    );

    Ok(memfd as u64)
}

fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

fn touch_pages(addr: *mut u8, length: usize) {
    let pagesz = page_size();
    let mut offset = 0usize;

    // Use volatile writes to avoid compiler optimizing the touches out
    while offset < length {
        unsafe {
            let p = addr.add(offset) as *mut u8;
            // read-modify-write one byte
            let _old = core::ptr::read_volatile(p);
            core::ptr::write_volatile(p, _old.wrapping_add(1));
        }
        offset += pagesz;
    }
}

fn read_pagemap_entry(pid: libc::pid_t, vaddr: usize) -> std::io::Result<u64> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    // open /proc/<pid>/pagemap and read 8 bytes at offset (vaddr / pagesize) * 8
    let path = format!("/proc/{}/pagemap", pid);
    let mut f = File::open(path)?;
    let pagesz = page_size();
    let index = (vaddr / pagesz) as u64;
    let off = (index * core::mem::size_of::<u64>() as u64) as u64;
    f.seek(SeekFrom::Start(off))?;
    let mut buf = [0u8; 8];
    f.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn parse_pagemap(entry: u64) -> (bool, u64) {
    // Using common interpretation:
    // bit 63 - page present
    // bits 0-54 - PFN (on many kernels)
    // (This matches common userland parsers; kernel docs: Documentation/vm/pagemap.txt)
    let present = (entry >> 63) & 1 != 0;
    let pfn_mask: u64 = (1u64 << 55) - 1;
    let pfn = entry & pfn_mask;
    (present, pfn)
}

fn print_pagemap_info(vaddr_start: usize, length: usize) {
    let pid = unsafe { libc::getpid() };
    let pagesz = page_size();

    let mut offset = 0usize;
    let mut last_printed_index: isize = -1;

    while offset < length {
        let va = vaddr_start + offset;

        if va % PAGE_SIZE_2M != 0 {
            offset += pagesz;
            continue;
        }

        let entry = match read_pagemap_entry(pid, va) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("failed reading pagemap for va {:#x}: {}", va, e);
                return;
            }
        };
        let (present, pfn) = parse_pagemap(entry);

        let page_index = (offset / pagesz) as isize;
        if page_index != last_printed_index {
            last_printed_index = page_index;
            println!(
                "VA {:#x} (+{:#x}) => present={} pfn={} gpa={:#x} (page_index={})",
                va,
                offset,
                present,
                pfn,
                pfn << 12,
                page_index
            );
        }

        offset += PAGE_SIZE_2M;
    }
}

/// Proxy for the `mmap` syscall to a `memfd`,
/// the daemon process will forward the `mmap` request to the Linux kernel
/// on behalf of the instance.
/// Most importantly, it will also locked the actual memfd's physical pages,
/// and sync the mapping with the instance through hypercalls.
///
/// Return the GPA in the instance address space on success.
pub fn sys_mmap_to_memfd(
    addr: u64,
    length: u64,
    prot: u64,
    flags: u64,
    fd: u64,
    offset: u64,
) -> LinuxResult<u64> {
    let memfd_name = if let Some(name) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        name.clone()
    } else {
        error!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };

    if !is_aligned_4k(offset as usize) {
        error!(
            "mmap to memfd with unaligned offset: {:#x}, fd: {}.",
            offset, fd
        );
        return Err(LinuxError::EINVAL);
    }

    info!(
        "mmap to \"{}\" addr: {:#x}, length: {:#x}, prot: {:#x}, flags: {:#x}, fd: {}, offset: {:#x}",
        memfd_name, addr, length, prot, flags, fd, offset
    );

    let res = unsafe {
        libc::mmap(
            addr as *mut libc::c_void,
            length as libc::size_t,
            prot as libc::c_int,
            flags as libc::c_int | MAP_LOCKED, // Use MAP_LOCKED to prevent swapping.
            fd as libc::c_int,
            offset as libc::off_t,
        )
    };

    if res == libc::MAP_FAILED {
        error!(
            "mmap to memfd failed with addr: {:#x}, length: {:#x}, prot: {:#x}, flags: {:#x}, fd: {}, offset: {:#x}, errno: {}",
            addr, length, prot, flags, fd, offset, std::io::Error::last_os_error()
        );
        return Err(LinuxError::ENOMEM);
    }

    if res != addr as *mut libc::c_void {
        error!(
            "mmap to memfd returned different address: requested {:#x}, got {:#p}",
            addr, res
        );
        // We expect the mmap to return the exact address we requested.
        // If not, something is wrong.
        unsafe {
            libc::munmap(res, length as libc::size_t);
        }
        return Err(LinuxError::EINVAL);
    }

    // Touch the pages to ensure they are mapped and locked.
    touch_pages(res as *mut u8, length as usize);

    // After pages are touched, read pagemap and print info for debugging.
    if false {
        print_pagemap_info(res as usize, length as usize);
    }

    unsafe {
        *(res as *mut usize) = 0xdeadbeef; // just to use the variable and avoid warnings
    }

    // Notify the hypervisor about the shared memory attachment.
    // This is necessary to sync the mapping with the instance.
    // The hypervisor will handle the actual mapping in the instance's address space.
    // The instance ID is used to identify the instance that is attaching the shared memory.
    // The hypervisor will return the GPA (guest physical address) of the shared memory region
    // which is then used by the instance to access the shared memory.
    // Just reuse the `hvc_daemon_shmat` function for simplicity.
    let res = hvc::hvc_daemon_shmat(instance_id() as u64, 1, fd, addr, length, flags);

    if res == -1 {
        error!(
            "Failed to attach shared memory for instance ID: {}, errno: {}",
            instance_id(),
            res
        );
        return Err(LinuxError::ENOMEM);
    }

    info!("Get instance shm_gpa {:#x}", res as usize);

    debug!(
        "Proxying mmap to memfd addr: {:#x}, length: {:#x}, prot: {:#x}, flags: {:#x}, fd: {}, offset: {:#x}, ret {:#x}",
        addr, length, prot, flags, fd, offset, res as u64
    );

    Ok(res as u64)
}
