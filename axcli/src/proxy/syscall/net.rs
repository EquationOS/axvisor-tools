//! Proxy for network operations.

use axerrno::LinuxResult;

use crate::proxy::FD_LIST;

pub fn proxy_socket(domain: u64, ty: u64, protocol: u64) -> LinuxResult<u64> {
    let fd = unsafe { libc::socket(domain as i32, ty as i32, protocol as i32) };

    if fd > 0 {
        debug!("Created socket with fd: {}", fd);

        FD_LIST.lock().unwrap().insert(fd, "socket".to_string());
    } else {
        error!(
            "Failed to create socket, error: {}",
            std::io::Error::last_os_error()
        );
    }

    debug!("Proxy socket created with fd: {}", fd);

    Ok(fd as u64)
}

pub fn proxy_connect(fd: u64, addr_ptr: u64, addr_len: u64) -> LinuxResult<u64> {
    let ret = unsafe {
        libc::connect(
            fd as i32,
            addr_ptr as *const libc::sockaddr,
            addr_len as u32,
        )
    };

    if ret < 0 {
        error!(
            "Failed to connect socket fd: {}, error: {}",
            fd,
            std::io::Error::last_os_error()
        );
    }

    debug!("Proxy connect called on fd: {}, result: {}", fd, ret);

    Ok(ret as u64)
}

pub fn proxy_sendmsg(fd: u64, msg_ptr: u64, flags: u64) -> LinuxResult<u64> {
    let ret = unsafe { libc::sendmsg(fd as i32, msg_ptr as *const libc::msghdr, flags as i32) };

    if ret < 0 {
        error!(
            "Failed to sendmsg on fd: {}, error: {}",
            fd,
            std::io::Error::last_os_error()
        );
    }

    warn!("Proxy sendmsg called on fd: {}, result: {}", fd, ret);

    Ok(ret as u64)
}

pub fn proxy_recvmsg(fd: u64, msg_ptr: u64, flags: u64) -> LinuxResult<u64> {
    let ret = unsafe { libc::recvmsg(fd as i32, msg_ptr as *mut libc::msghdr, flags as i32) };

    if ret < 0 {
        error!(
            "Failed to recvmsg on fd: {}, error: {}",
            fd,
            std::io::Error::last_os_error()
        );
    }

    warn!("Proxy recvmsg called on fd: {}, result: {}", fd, ret);

    Ok(ret as u64)
}
