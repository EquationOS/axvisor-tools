use axerrno::LinuxResult;

pub fn sys_shmget(key: u64, size: u64, shmflg: u64) -> LinuxResult<u64> {
    let res = unsafe {
        libc::shmget(
            key as libc::key_t,
            size as libc::size_t,
            shmflg as libc::c_int,
        )
    };

    debug!(
        "Proxying shmget key: {:#x}, size: {:#x}, flags: {:#x}, ret {}",
        key, size, shmflg, res as u64
    );

    Ok(res as u64)
}
