use axerrno::LinuxResult;

pub fn sys_getrandom(buf: u64, buflen: u64, flags: u64) -> LinuxResult<u64> {
    let res = unsafe { libc::getrandom(buf as *mut libc::c_void, buflen as usize, flags as u32) };

    Ok(res as u64)
}

pub fn sys_arch_prctl(op: u64, addr: u64) -> LinuxResult<u64> {
    let res = unsafe { super::raw_syscall::arch_prctl(op, addr) };
    Ok(res as _)
}
