//! Memory management related proxy functions.
//!
//! We need to find a way to manage the address space layout of multiple
//! processes fron one instance.

// TODO: move instance related mmap functions here.

use libc::mmap;

use crate::proxy::INSTANCE_FD;

pub fn instance_mmap() {}
