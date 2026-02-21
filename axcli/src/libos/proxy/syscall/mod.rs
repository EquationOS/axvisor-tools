//! This module contains the syscall proxy implementations for various system calls.

pub(super) mod fs;
pub(super) mod misc;
pub(super) mod net;
pub(super) mod shm;

mod raw_syscall;
