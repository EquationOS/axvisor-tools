// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::os::unix::fs::MetadataExt;

use axerrno::{AxResult, ax_err, ax_err_type};
use memory_addr::{PAGE_SIZE_4K, align_down};
use vm_memory::{
    Address, GuestAddress, GuestMemory, GuestMemoryRegion, ReadVolatile, VolatileMemoryError,
};

use crate::microvm::config::BootConfig;
use crate::microvm::vstate::memory::GuestMemoryMmap;
use crate::utils::u64_to_usize;

/// Returns the memory address where the initrd could be loaded.
pub fn initrd_load_addr(guest_mem: &GuestMemoryMmap, initrd_size: usize) -> Option<u64> {
    let first_region = guest_mem.find_region(GuestAddress::new(0))?;
    let lowmem_size = u64_to_usize(first_region.len());

    if lowmem_size < initrd_size {
        return None;
    }

    Some(align_down(lowmem_size - initrd_size, PAGE_SIZE_4K) as u64)
}

/// Type for passing information about the initrd in the guest memory.
#[derive(Debug)]
pub struct InitrdConfig {
    /// Load address of initrd in guest memory
    pub address: GuestAddress,
    /// Size of initrd in guest memory
    pub size: usize,
}

impl InitrdConfig {
    /// Load initrd into guest memory based on the boot config.
    pub fn from_config(
        boot_cfg: &BootConfig,
        vm_memory: &GuestMemoryMmap,
    ) -> AxResult<Option<Self>> {
        Ok(match &boot_cfg.initrd_file {
            Some(f) => {
                let f = f.try_clone().map_err(|e| {
                    ax_err_type!(
                        InvalidInput,
                        format_args!("failed to clone initrd file handle: {}", e)
                    )
                })?;
                Some(Self::from_file(vm_memory, f)?)
            }
            None => None,
        })
    }

    /// Loads the initrd from a file into guest memory.
    pub fn from_file(vm_memory: &GuestMemoryMmap, mut file: File) -> AxResult<Self> {
        let size = file
            .metadata()
            .map_err(|e| {
                ax_err_type!(
                    InvalidInput,
                    format_args!("failed to get initrd file metadata: {}", e)
                )
            })?
            .size();
        let size = u64_to_usize(size);
        let Some(address) = initrd_load_addr(vm_memory, size) else {
            return ax_err!(BadAddress, "Boot initrd image does not fit in guest memory");
        };
        let mut slice = vm_memory
            .get_slice(GuestAddress(address), size)
            .map_err(|e| {
                ax_err_type!(
                    BadAddress,
                    format_args!("Failed to get guest memory slice for initrd, err {}", e)
                )
            })?;
        file.read_exact_volatile(&mut slice).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!("failed to read initrd file: {}", e)
            )
        })?;

        Ok(InitrdConfig {
            address: GuestAddress(address),
            size,
        })
    }
}
