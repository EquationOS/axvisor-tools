// Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::convert::Infallible;

use serde::{Deserialize, Serialize};
pub use vm_allocator::AllocPolicy;
use vm_allocator::{AddressAllocator, IdAllocator};

use crate::microvm::arch;

/// Helper function to allocate many ids from an id allocator
fn allocate_many_ids(
    id_allocator: &mut IdAllocator,
    count: u32,
) -> Result<Vec<u32>, vm_allocator::Error> {
    let mut ids = Vec::with_capacity(count as usize);

    for _ in 0..count {
        match id_allocator.allocate_id() {
            Ok(id) => ids.push(id),
            Err(err) => {
                // It is ok to unwrap here, we just allocated the GSI
                ids.into_iter().for_each(|id| {
                    id_allocator.free_id(id).unwrap();
                });
                return Err(err);
            }
        }
    }

    Ok(ids)
}

/// A resource manager for (de)allocating interrupt lines (GSIs) and guest memory
///
/// At the moment, we support:
///
/// * GSIs for legacy x86_64 devices
/// * GSIs for MMIO devicecs
/// * Memory allocations in the MMIO address space
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAllocator {
    /// Allocator for legacy device interrupt lines
    pub gsi_legacy_allocator: IdAllocator,
    /// Allocator for PCI device GSIs
    pub gsi_msi_allocator: IdAllocator,
    /// Allocator for memory in the 32-bit MMIO address space
    pub mmio32_memory: AddressAllocator,
    /// Allocator for memory in the 64-bit MMIO address space
    pub mmio64_memory: AddressAllocator,
    /// Allocator for memory after the 64-bit MMIO address space
    pub past_mmio64_memory: AddressAllocator,
    /// Memory allocator for system data
    pub system_memory: AddressAllocator,
}

impl Default for ResourceAllocator {
    fn default() -> Self {
        ResourceAllocator::new()
    }
}

impl ResourceAllocator {
    /// Create a new resource allocator for Firecracker devices
    pub fn new() -> Self {
        // It is fine for us to unwrap the following since we know we are passing valid ranges for
        // all allocators
        Self {
            gsi_legacy_allocator: IdAllocator::new(arch::GSI_LEGACY_START, arch::GSI_LEGACY_END)
                .unwrap(),
            gsi_msi_allocator: IdAllocator::new(arch::GSI_MSI_START, arch::GSI_MSI_END).unwrap(),
            mmio32_memory: AddressAllocator::new(
                arch::MEM_32BIT_DEVICES_START,
                arch::MEM_32BIT_DEVICES_SIZE,
            )
            .unwrap(),
            mmio64_memory: AddressAllocator::new(
                arch::MEM_64BIT_DEVICES_START,
                arch::MEM_64BIT_DEVICES_SIZE,
            )
            .unwrap(),
            past_mmio64_memory: AddressAllocator::new(
                arch::FIRST_ADDR_PAST_64BITS_MMIO,
                arch::PAST_64BITS_MMIO_SIZE,
            )
            .unwrap(),
            system_memory: AddressAllocator::new(arch::SYSTEM_MEM_START, arch::SYSTEM_MEM_SIZE)
                .unwrap(),
        }
    }

    /// Allocate a number of legacy GSIs
    ///
    /// # Arguments
    ///
    /// * `gsi_count` - The number of legacy GSIs to allocate
    pub fn allocate_gsi_legacy(&mut self, gsi_count: u32) -> Result<Vec<u32>, vm_allocator::Error> {
        allocate_many_ids(&mut self.gsi_legacy_allocator, gsi_count)
    }

    /// Allocate a number of GSIs for MSI
    ///
    /// # Arguments
    ///
    /// * `gsi_count` - The number of GSIs to allocate
    pub fn allocate_gsi_msi(&mut self, gsi_count: u32) -> Result<Vec<u32>, vm_allocator::Error> {
        allocate_many_ids(&mut self.gsi_msi_allocator, gsi_count)
    }

    /// Allocate a memory range in 32-bit MMIO address space
    ///
    /// If it succeeds, it returns the first address of the allocated range
    ///
    /// # Arguments
    ///
    /// * `size` - The size in bytes of the memory to allocate
    /// * `alignment` - The alignment of the address of the first byte
    /// * `policy` - A [`vm_allocator::AllocPolicy`] variant for determining the allocation policy
    pub fn allocate_32bit_mmio_memory(
        &mut self,
        size: u64,
        alignment: u64,
        policy: AllocPolicy,
    ) -> Result<u64, vm_allocator::Error> {
        Ok(self
            .mmio32_memory
            .allocate(size, alignment, policy)?
            .start())
    }

    /// Allocate a memory range in 64-bit MMIO address space
    ///
    /// If it succeeds, it returns the first address of the allocated range
    ///
    /// # Arguments
    ///
    /// * `size` - The size in bytes of the memory to allocate
    /// * `alignment` - The alignment of the address of the first byte
    /// * `policy` - A [`vm_allocator::AllocPolicy`] variant for determining the allocation policy
    pub fn allocate_64bit_mmio_memory(
        &mut self,
        size: u64,
        alignment: u64,
        policy: AllocPolicy,
    ) -> Result<u64, vm_allocator::Error> {
        Ok(self
            .mmio64_memory
            .allocate(size, alignment, policy)?
            .start())
    }

    /// Allocate a memory range for system data
    ///
    /// If it succeeds, it returns the first address of the allocated range
    ///
    /// # Arguments
    ///
    /// * `size` - The size in bytes of the memory to allocate
    /// * `alignment` - The alignment of the address of the first byte
    /// * `policy` - A [`vm_allocator::AllocPolicy`] variant for determining the allocation policy
    pub fn allocate_system_memory(
        &mut self,
        size: u64,
        alignment: u64,
        policy: AllocPolicy,
    ) -> Result<u64, vm_allocator::Error> {
        Ok(self
            .system_memory
            .allocate(size, alignment, policy)?
            .start())
    }
}
