//! Raw syscall implementations.
//! Refer to <https://github.com/saethlin/veneer/blob/main/src/syscalls.rs>

use sc::syscall;

#[inline]
pub unsafe fn getdents64(fd: libc::c_int, buf: &mut [u8]) -> usize {
    unsafe { syscall!(GETDENTS64, fd, buf.as_mut_ptr(), buf.len()) }
}

#[inline]
pub unsafe fn arch_prctl(op: u64, addr: u64) -> usize {
    unsafe { syscall!(ARCH_PRCTL, op, addr) }
}
