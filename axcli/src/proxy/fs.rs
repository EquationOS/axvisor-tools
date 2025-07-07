use std::{collections::BTreeMap, sync::Mutex};

use axerrno::{LinuxError, LinuxResult};

use super::INSTANCE_FD;

static FD_LIST: Mutex<BTreeMap<i32, String>> = Mutex::new(BTreeMap::new());

pub fn proxy_access(path_ptr: u64, mode: u64) -> LinuxResult<u64> {
    // Convert the path pointer to a Rust string
    let path = unsafe {
        let cstr = std::ffi::CStr::from_ptr(path_ptr as *const i8);
        cstr.to_string_lossy().into_owned()
    };

    info!("Accessing path: \"{}\" mode: {:#x}", path, mode);

    // Call the actual filesystem access function
    Ok(unsafe { libc::access(path_ptr as *const i8, mode as i32) } as u64)
}

pub fn proxy_openat(dirfd: u64, pathname_ptr: u64, flags: u64, mode: u64) -> LinuxResult<u64> {
    // Convert the pathname pointer to a Rust string
    let pathname = unsafe {
        let cstr = std::ffi::CStr::from_ptr(pathname_ptr as *const i8);
        cstr.to_string_lossy().into_owned()
    };

    info!(
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
        info!("Opened file descriptor {} for path: {}", fd, pathname);
        // Store the file descriptor and its path in the FD_LIST
        FD_LIST.lock().unwrap().insert(fd, pathname);
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
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        info!("File descriptor {} corresponds to path: {}", fd, path);
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }

    info!(
        "Proxying fstat syscall for fd: {}, stat_ptr: {:#x}",
        fd, stat_ptr
    );

    // Call the actual filesystem fstat function
    let ret = unsafe { libc::fstat(fd as i32, stat_ptr as *mut libc::stat) };

    Ok(ret as u64)
}

pub fn proxy_close(fd: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    if let Some(path) = FD_LIST.lock().unwrap().remove(&(fd as i32)) {
        info!("Proxying close syscall for fd {} for path: {}", fd, path);
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }

    // Call the actual filesystem close function
    let ret = unsafe { libc::close(fd as i32) };

    Ok(ret as u64)
}

pub fn proxy_write(fd: u64, buf_ptr: u64, count: u64) -> LinuxResult<u64> {
    // Check if the file descriptor exists in the FD_LIST
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        info!("File descriptor {} corresponds to path: {}", fd, path);
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }

    info!(
        "Proxying write syscall for fd: {}, buf_ptr: {:#x}, count: {}, content: {:?}",
        fd,
        buf_ptr,
        count,
        escape_c_string_style(unsafe { std::slice::from_raw_parts(buf_ptr as *const u8, 20) })
    );

    // Call the actual filesystem write function
    let ret = unsafe { libc::write(fd as i32, buf_ptr as *const libc::c_void, count as usize) };

    debug!("Wrote {} bytes to fd: {}, buf_ptr: {:#x}", ret, fd, buf_ptr);

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
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        info!("File descriptor {} corresponds to path: {}", fd, path);
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }
    info!(
        "Proxying read syscall for fd: {}, buf_ptr: {:#x}, count: {}",
        fd, buf_ptr, count
    );

    // Call the actual filesystem read function
    let ret = unsafe { libc::read(fd as i32, buf_ptr as *mut libc::c_void, count as usize) };

    debug!(
        "Read {} bytes from fd: {}, buf_ptr: {:#x}, content [{:?}]",
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

pub fn proxy_mmap(
    addr: u64,
    length: u64,
    prot: u64,
    flags: u64,
    fd: u64,
    offset: u64,
) -> LinuxResult<u64> {
    info!(
        "Proxying mmap syscall for addr: {:#x}, length: {:#x}, prot: {:#x}, flags: {:#x}, fd: {}, offset: {:#x}",
        addr, length, prot, flags, fd, offset
    );

    let mut file_len;
    // Check if the file descriptor exists in the FD_LIST,
    // we only handle mmap for file descriptors that are already registered
    // in the FD_LIST.
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        info!("File descriptor {} corresponds to path: {}", fd, path);

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

    let host_file_mem = unsafe {
        libc::mmap(
            0 as *mut libc::c_void, // null
            length as usize,
            libc::PROT_READ,
            flags as i32 & !libc::MAP_FIXED, // Remove MAP_FIXED to avoid conflicts
            fd as i32,
            offset as libc::off_t,
        )
    };

    if host_file_mem == libc::MAP_FAILED {
        error!(
        "Proxying mmap syscall for addr: {:#x}, length: {:#x}, prot: {:#x}, flags: {:#x}, fd: {}, offset: {:#x} failed with error: {}",
        addr, length, prot, flags, fd, offset, std::io::Error::last_os_error()
        );
        return Ok(libc::MAP_FAILED as u64);
    }

    debug!(
        "Host file memory mapped at: {:#x} for fd: {}, length: {}, offset {:#x}",
        host_file_mem as u64, fd, length, offset
    );

    // We do not actually mmap to the file here,
    // instead, we mmap the requested virtual address to the page cache,
    // and copy the file content to the page cache that we maintain.
    let ret = unsafe {
        libc::mmap(
            addr as *mut libc::c_void,
            length as usize,
            prot as i32,
            libc::MAP_SHARED | libc::MAP_FIXED,
            INSTANCE_FD,
            0, // We use 0 offset for the instance FD
        )
    };

    if file_len < offset {
        error!(
            "Offset {:#x} is greater than file length {}, cannot mmap",
            offset, file_len
        );
        return Err(LinuxError::EINVAL);
    }
    file_len -= offset; // Adjust file_len to account for the offset

    if ret == libc::MAP_FAILED {
        error!(
            "mmap failed with error: {}",
            std::io::Error::last_os_error()
        );
        return Ok(libc::MAP_FAILED as u64);
    }

    let copied_length = if file_len < length { file_len } else { length };

    debug!(
        "Memory mapped at: {:#x}, copied length: {}",
        ret as u64, copied_length
    );

    unsafe {
        libc::memcpy(
            ret as *mut libc::c_void,
            host_file_mem,
            copied_length as usize,
        );
    }
    debug!(
        "Copied {:#x}({}) bytes from host file memory {:#x} to mapped memory [{:#x}~{:#x}]",
        copied_length,
        copied_length,
        host_file_mem as u64,
        ret as u64,
        ret as u64 + copied_length as u64
    );

    if copied_length < length {
        // If the file is shorter than the requested length, zero out the remaining bytes
        let remaining_length = length - copied_length;
        let zero_ptr = (ret as usize + copied_length as usize) as *mut libc::c_void;
        debug!(
            "Zeroing out remaining {:#x}({}) bytes at [{:#x}~{:#x}]",
            remaining_length,
            remaining_length,
            zero_ptr as u64,
            zero_ptr as u64 + remaining_length as u64
        );

        unsafe {
            libc::memset(zero_ptr, 0, remaining_length as usize);
        }
    }

    unsafe {
        libc::munmap(host_file_mem, length as usize);
    }

    Ok(ret as u64)
}

pub fn proxy_pread64(fd: u64, buf_ptr: u64, count: u64, offset: u64) -> LinuxResult<u64> {
    info!(
        "Proxying pread64 syscall for fd: {}, buf_ptr: {:#x}, count: {}, offset: {:#x}",
        fd, buf_ptr, count, offset
    );

    // Check if the file descriptor exists in the FD_LIST
    if let Some(path) = FD_LIST.lock().unwrap().get(&(fd as i32)) {
        info!("File descriptor {} corresponds to path: {}", fd, path);
    } else {
        warn!("File descriptor {} not found in FD_LIST", fd);
        return Err(LinuxError::ENOENT);
    }

    // Call the actual filesystem pread64 function
    let ret = unsafe {
        libc::pread64(
            fd as i32,
            buf_ptr as *mut libc::c_void,
            count as usize,
            offset as libc::off_t,
        )
    };

    if ret < 0 {
        error!(
            "pread64 failed with error: {}",
            std::io::Error::last_os_error()
        );
    }

    Ok(ret as u64)
}
