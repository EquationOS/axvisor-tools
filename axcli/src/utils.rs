use axerrno::AxResult;

/// How many bits to left-shift by to convert MiB to bytes
const MIB_TO_BYTES_SHIFT: usize = 20;

/// Return the default page size of the platform, in bytes.
pub fn get_page_size() -> AxResult<usize> {
    // SAFETY: Safe because the parameters are valid.
    match unsafe { libc::sysconf(libc::_SC_PAGESIZE) } {
        -1 => Err(axerrno::ax_err_type!(
            BadState,
            "Failed to get the system page size"
        )),
        ps => Ok(usize::try_from(ps).unwrap()),
    }
}

/// Safely converts a u64 value to a usize value.
/// This bypasses the Clippy lint check because we only support 64-bit platforms.
#[cfg(target_pointer_width = "64")]
#[inline]
#[allow(clippy::cast_possible_truncation)]
pub const fn u64_to_usize(num: u64) -> usize {
    num as usize
}

/// Safely converts a usize value to a u64 value.
/// This bypasses the Clippy lint check because we only support 64-bit platforms.
#[cfg(target_pointer_width = "64")]
#[inline]
#[allow(clippy::cast_possible_truncation)]
pub const fn usize_to_u64(num: usize) -> u64 {
    num as u64
}

/// Converts MiB to Bytes
pub const fn mib_to_bytes(mib: usize) -> usize {
    mib << MIB_TO_BYTES_SHIFT
}
