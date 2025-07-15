use std::{collections::BTreeMap, sync::Mutex};

use axerrno::{LinuxError, LinuxResult};
use libc::MAP_PRIVATE;

use equation_defs::{PAGE_CACHE_POOL_BASE_VA, PAGE_CACHE_POOL_SIZE};

static FD_LIST: Mutex<BTreeMap<i32, String>> = Mutex::new(BTreeMap::new());

pub fn proxy_access(path_ptr: u64, mode: u64) -> LinuxResult<u64> {
    // Convert the path pointer to a Rust string
    let path = unsafe {
        let cstr = std::ffi::CStr::from_ptr(path_ptr as *const i8);
        cstr.to_string_lossy().into_owned()
    };

    debug!("Proxying access syscall for path: \"{}\" mode: {:#x}", path, mode);

    // Call the actual filesystem access function
    Ok(unsafe { libc::access(path_ptr as *const i8, mode as i32) } as u64)
}

/// Proxy the `openat` syscall to handle file opening operations,
/// I made a little extension to the original `openat` syscall,
/// it can also handle the `stat` operation by passing a pointer to a `stat`
/// structure as the last argument.
///
/// If the 5th argument `stat_ptr` is not 0, it will fill the `stat` structure with the file information.
pub fn proxy_openat_with_stat(
    dirfd: u64,
    pathname_ptr: u64,
    flags: u64,
    mode: u64,
    stat_ptr: u64,
) -> LinuxResult<u64> {
    // Convert the pathname pointer to a Rust string
    let pathname = unsafe {
        let cstr = std::ffi::CStr::from_ptr(pathname_ptr as *const i8);
        cstr.to_string_lossy().into_owned()
    };

    debug!(
        "Proxying openat syscall for dirfd: {:#x}, path: \"{}\", flags: {:#x}, mode: {:#x}",
        dirfd, pathname, flags, mode
    );

    // Call the actual filesystem openat function
    let fd = unsafe {
        libc::openat(
            dirfd as i32,
            pathname_ptr as *const i8,
            flags as i32,
            mode as i32,
        )
    };

    if fd > 0 {
        trace!("Opened file descriptor {} for path: {}", fd, pathname);
        // Store the file descriptor and its path in the FD_LIST
        FD_LIST.lock().unwrap().insert(fd, pathname);

        // If stat_ptr is provided, fill the stat structure
        if stat_ptr != 0 {
            proxy_fstat(fd as _, stat_ptr)?;
        }
    } else {
        error!(
            "Failed to open file at path: {}, fd: {}, error {}",
            pathname,
            fd,
            std::io::Error::last_os_error()
        );
    }

    Ok(fd as u64)
}

pub fn proxy_fstat(fd: u64, stat_ptr: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    let path: String = if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        trace!("File descriptor {} corresponds to path: {}", fd, path);
        path.clone()
    } else if (0..=2).contains(&(fd as i32)) {
        // Special case for stdin, stdout, stderr
        trace!(
            "File descriptor {} is a special file (stdin/stdout/stderr)",
            fd
        );
        match fd {
            0 => String::from("stdin"),
            1 => String::from("stdout"),
            2 => String::from("stderr"),
            _ => unreachable!(),
        }
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };

    debug!("Proxying fstat syscall for fd:{fd} \"{path}\" stat_ptr: {stat_ptr:#x}",);

    // Call the actual filesystem fstat function
    let ret = unsafe { libc::fstat(fd as i32, stat_ptr as *mut libc::stat) };

    Ok(ret as u64)
}

pub fn proxy_newfstatat(
    dirfd: u64,
    pathname_ptr: u64,
    stat_ptr: u64,
    flags: u64,
) -> LinuxResult<u64> {
    // Convert the pathname pointer to a Rust string
    let pathname = unsafe {
        let cstr = std::ffi::CStr::from_ptr(pathname_ptr as *const i8);
        cstr.to_string_lossy().into_owned()
    };

    debug!(
        "Proxying newfstatat syscall for dirfd: {:#x}, path: \"{}\", stat_ptr: {:#x}, flags: {:#x}",
        dirfd, pathname, stat_ptr, flags
    );

    // Call the actual filesystem newfstatat function
    let ret = unsafe {
        libc::fstatat(
            dirfd as i32,
            pathname_ptr as *const i8,
            stat_ptr as *mut libc::stat,
            flags as i32,
        )
    };

    if ret < 0 {
        error!(
            "Failed to get file status for path: {}, error {}",
            pathname,
            std::io::Error::last_os_error()
        );
    }

    Ok(ret as u64)
}

pub fn proxy_close(fd: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    let path = if let Some(path) = FD_LIST.lock().unwrap().remove(&(fd as i32)) {
        path
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };

    // Call the actual filesystem close function
    let ret = unsafe { libc::close(fd as i32) };

    debug!(
        "Proxying close syscall for fd: {}, path: \"{}\", result: {}",
        fd, path, ret
    );

    Ok(ret as u64)
}

pub fn proxy_write(fd: u64, buf_ptr: u64, count: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    let path = if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        trace!("File descriptor {} corresponds to path: {}", fd, path);
        path.clone()
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };

    trace!(
        "Proxying write syscall for fd: {}, buf_ptr: {:#x}, count: {}, content: {:?}",
        fd,
        buf_ptr,
        count,
        escape_c_string_style(unsafe { std::slice::from_raw_parts(buf_ptr as *const u8, 20) })
    );

    // Call the actual filesystem write function
    let ret = unsafe { libc::write(fd as i32, buf_ptr as *const libc::c_void, count as usize) };

    debug!(
        "Wrote {} bytes to fd:{}, path \"{path}\" buf_ptr: {:#x}",
        ret, fd, buf_ptr
    );

    if ret < 0 {
        error!(
            "Write failed with error: {}",
            std::io::Error::last_os_error()
        );
        return Err(LinuxError::EIO);
    }

    Ok(ret as u64)
}

fn escape_c_string_style(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        if b.is_ascii_graphic() || b == b' ' {
            s.push(b as char);
        } else {
            s.push_str(&format!("\\{:03o}", b)); // 使用八进制转义
        }
    }
    s
}

pub fn proxy_read(fd: u64, buf_ptr: u64, count: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    let path = if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        trace!("File descriptor {} corresponds to path: {}", fd, path);
        path.clone()
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };
    trace!(
        "Proxying read syscall for fd: {}, buf_ptr: {:#x}, count: {}",
        fd,
        buf_ptr,
        count
    );

    // Call the actual filesystem read function
    let ret = unsafe { libc::read(fd as i32, buf_ptr as *mut libc::c_void, count as usize) };

    debug!(
        "Read {} bytes from fd:{} \"{path}\" buf_ptr:{:#x}, content [{:?}]",
        ret,
        fd,
        buf_ptr,
        escape_c_string_style(unsafe { std::slice::from_raw_parts(buf_ptr as *const u8, 20) })
    );

    if ret < 0 {
        error!(
            "Read failed with error: {}",
            std::io::Error::last_os_error()
        );
        return Err(LinuxError::EIO);
    }

    Ok(ret as u64)
}

/// Proxy mmap syscall to copy file contents into the page cache region,
/// yes, I reuse the arguments of the mmap syscall to pass the parameters,
/// with the following meanings:
/// - `addr`: The address in the page cache region where the file content should be copied to
/// - `length`: The length of the file content to be copied
/// - `wb_fd`: The file descriptor for writeback, if not used, set to 0
/// - `wb_offset`: The offset in the writeback file, if not used, set to 0
/// - `fd`: The file descriptor of the file to be copied
/// - `offset`: The offset in the file to start copying from
///
pub fn proxy_mmap_into_pagecache(
    addr: u64,
    length: u64,
    wb_fd: u64,     // prot
    wb_offset: u64, // flags
    fd: u64,
    offset: u64,
) -> LinuxResult<u64> {
    trace!(
        "Proxying mmap syscall for addr: {:#x}, length: {:#x}, wb_fd: {:#x}, wb_offset: {:#x}, fd: {}, offset: {:#x}",
        addr, length, wb_fd, wb_offset, fd, offset
    );

    if wb_fd != 0 {
        warn!("Writeback {length} Bytes from {addr:#x} to {wb_fd} at offset {wb_offset:#x}, not supported yet");
        return Err(LinuxError::ENOSYS);
    }

    if !(PAGE_CACHE_POOL_BASE_VA..PAGE_CACHE_POOL_BASE_VA + PAGE_CACHE_POOL_SIZE)
        .contains(&(addr as usize))
    {
        error!(
            "Address {:#x} is not within the page cache pool region [{:#x}~{:#x}]",
            addr,
            PAGE_CACHE_POOL_BASE_VA,
            PAGE_CACHE_POOL_BASE_VA + PAGE_CACHE_POOL_SIZE
        );
        return Err(LinuxError::EINVAL);
    }

    let mut file_len;
    // Check if the file descriptor exists in the FD_LIST,
    // we only handle mmap for file descriptors that are already registered
    // in the FD_LIST.
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        // Update the file length to prevent memcpy from reading beyond the file size.
        let file = std::fs::File::open(path).map_err(|e| {
            error!("Failed to open file {}: {}", path, e);
            LinuxError::ENOENT
        })?;
        let file_size = file
            .metadata()
            .map_err(|e| {
                error!("Failed to get metadata for file {}: {}", path, e);
                LinuxError::ENOENT
            })?
            .len();

        file_len = file_size;
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }

    // If the passed `offset` is already greater than or equal to the file length,
    // we can just zero out the target frame in the page cache pool.
    if offset >= file_len {
        // Just zero the target frame in page cache pool.
        trace!(
            "Offset {:#x} is greater than or equal to file length {}, zeroing out page cache pool at {:#x} with length {:#x}",
            offset, file_len, addr, length
        );
        unsafe {
            libc::memset(addr as *mut libc::c_void, 0, length as usize);
        }
        let checksum = calculate_checksum(unsafe {
            core::slice::from_raw_parts(addr as *const u8, length as usize)
        });

        return Ok(checksum as u64);
    }

    let host_file_mem = unsafe {
        libc::mmap(
            0 as *mut libc::c_void, // null
            length as usize,
            libc::PROT_READ,
            MAP_PRIVATE,
            fd as i32,
            offset as libc::off_t,
        )
    };

    if host_file_mem == libc::MAP_FAILED {
        error!(
        "Proxying mmap syscall for addr: {:#x}, length: {:#x}, fd: {}, offset: {:#x} failed with error: {}",
        addr, length, fd, offset, std::io::Error::last_os_error()
        );
        return Ok(libc::MAP_FAILED as u64);
    }

    // Now, copy the content from the host memory to the page cache region at the given address.

    if file_len < offset {
        error!(
            "Offset {:#x} is greater than file length {}, cannot mmap",
            offset, file_len
        );
        return Err(LinuxError::EINVAL);
    }
    file_len -= offset; // Adjust file_len to account for the offset

    let copied_length = if file_len < length { file_len } else { length };

    unsafe {
        libc::memcpy(
            addr as *mut libc::c_void,
            host_file_mem,
            copied_length as usize,
        );
    }

    if copied_length < length {
        // If the file is shorter than the requested length, zero out the remaining bytes
        let remaining_length = length - copied_length;
        let zero_ptr = (addr as usize + copied_length as usize) as *mut libc::c_void;
        unsafe {
            libc::memset(zero_ptr, 0, remaining_length as usize);
        }
    }

    unsafe {
        libc::munmap(host_file_mem, length as usize);
    }

    use equation_defs::page_cache::checksum::calculate_checksum;

    let checksum = calculate_checksum(unsafe {
        core::slice::from_raw_parts(addr as *const u8, length as usize)
    });
    Ok(checksum as u64)
}

pub fn proxy_pread64(fd: u64, buf_ptr: u64, count: u64, offset: u64) -> LinuxResult<u64> {
    trace!(
        "Proxying pread64 syscall for fd: {}, buf_ptr: {:#x}, count: {}, offset: {:#x}",
        fd,
        buf_ptr,
        count,
        offset
    );

    // Check if the file descriptor exists in the FD_LIST
    let path = if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        path.clone()
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    };

    // Call the actual filesystem pread64 function
    let ret = unsafe {
        libc::pread64(
            fd as i32,
            buf_ptr as *mut libc::c_void,
            count as usize,
            offset as libc::off_t,
        )
    };

    debug!(
        "Read {} bytes from fd:{}, path \"{}\" buf_ptr: {:#x}, offset: {:#x}",
        ret, fd, path, buf_ptr, offset
    );

    if ret < 0 {
        error!(
            "pread64 failed with error: {}",
            std::io::Error::last_os_error()
        );
    }

    Ok(ret as u64)
}
