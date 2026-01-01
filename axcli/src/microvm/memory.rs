// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::fs::File;
use std::io::SeekFrom;
use std::ops::Deref;
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};

use axerrno::{AxResult, ax_err, ax_err_type};
use bitvec::vec::BitVec;
use serde::{Deserialize, Serialize};

use crate::microvm::layout::{
    MMIO32_MEM_SIZE, MMIO32_MEM_START, MMIO64_MEM_SIZE, MMIO64_MEM_START,
};
use crate::utils::u64_to_usize;

pub use vm_memory::bitmap::{AtomicBitmap, BS, Bitmap, BitmapSlice};
pub use vm_memory::mmap::MmapRegionBuilder;
use vm_memory::mmap::{MmapRegionError, NewBitmap};
pub use vm_memory::{
    Address, ByteValued, Bytes, FileOffset, GuestAddress, GuestMemory, GuestMemoryRegion,
    GuestUsize, MemoryRegionAddress, MmapRegion, address,
};
use vm_memory::{GuestMemoryError, GuestMemoryRegionBytes, VolatileSlice, WriteVolatile};

/// Type of GuestRegionMmap.
pub type GuestRegionMmap = vm_memory::GuestRegionMmap<Option<AtomicBitmap>>;
/// Type of GuestMemoryMmap.
pub type GuestMemoryMmap = vm_memory::GuestRegionCollection<GuestRegionMmapExt>;
/// Type of GuestMmapRegion.
pub type GuestMmapRegion = vm_memory::MmapRegion<Option<AtomicBitmap>>;

/// Type of the guest region
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GuestRegionType {
    /// Guest DRAM
    Dram,
    /// Hotpluggable memory
    Hotpluggable,
}

/// An extension to GuestMemoryRegion that can be split into multiple KVM slots of
/// the same slot_size, and stores the type of region, and the starting KVM slot number.
#[derive(Debug)]
pub struct GuestRegionMmapExt {
    /// the wrapped GuestRegionMmap
    pub inner: GuestRegionMmap,
    // /// the type of region
    // pub region_type: GuestRegionType,
    // /// the starting KVM slot number assigned to this region
    // pub slot_from: u32,
    // /// the size of the slots of this region
    // pub slot_size: usize,
    // /// a bitvec indicating whether slot `i` is plugged into KVM (1) or not (0)
    // pub plugged: Mutex<BitVec>,
}

impl Deref for GuestRegionMmapExt {
    type Target = MmapRegion<Option<AtomicBitmap>>;

    fn deref(&self) -> &MmapRegion<Option<AtomicBitmap>> {
        &self.inner
    }
}

impl GuestMemoryRegionBytes for GuestRegionMmapExt {}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_possible_truncation)]
impl GuestMemoryRegion for GuestRegionMmapExt {
    type B = Option<AtomicBitmap>;

    fn len(&self) -> GuestUsize {
        self.inner.len()
    }

    fn start_addr(&self) -> GuestAddress {
        self.inner.start_addr()
    }

    fn bitmap(&self) -> BS<'_, Self::B> {
        self.inner.bitmap()
    }

    fn get_host_address(
        &self,
        addr: MemoryRegionAddress,
    ) -> vm_memory::guest_memory::Result<*mut u8> {
        self.inner.get_host_address(addr)
    }

    fn file_offset(&self) -> Option<&FileOffset> {
        self.inner.file_offset()
    }

    fn get_slice(
        &self,
        offset: MemoryRegionAddress,
        count: usize,
    ) -> vm_memory::guest_memory::Result<VolatileSlice<'_, BS<'_, Self::B>>> {
        self.inner.get_slice(offset, count)
    }
}

impl GuestRegionMmapExt {
    /// Adds a DRAM region which only contains a single plugged slot
    pub(crate) fn dram_from_mmap_region(region: GuestRegionMmap) -> Self {
        // let slot_size = u64_to_usize(region.len());
        GuestRegionMmapExt {
            inner: region,
            // slot_from: slot,
            // slot_size,
            // plugged: Mutex::new(BitVec::repeat(true, 1)),
        }
    }
}

/// Adds in [`regions`] the valid memory regions suitable for RAM taking into account a gap in the
/// available address space and returns the remaining region (if any) past this gap
fn arch_memory_regions_with_gap(
    regions: &mut Vec<(GuestAddress, usize)>,
    region_start: usize,
    region_size: usize,
    gap_start: usize,
    gap_size: usize,
) -> Option<(usize, usize)> {
    // 0-sized gaps don't really make sense. We should never receive such a gap.
    assert!(gap_size > 0);

    let first_addr_past_gap = gap_start + gap_size;
    match (region_start + region_size).checked_sub(gap_start) {
        // case0: region fits all before gap
        None | Some(0) => {
            regions.push((GuestAddress(region_start as u64), region_size));
            None
        }
        // case1: region starts before the gap and goes past it
        Some(remaining) if region_start < gap_start => {
            regions.push((GuestAddress(region_start as u64), gap_start - region_start));
            Some((first_addr_past_gap, remaining))
        }
        // case2: region starts past the gap
        Some(_) => Some((first_addr_past_gap.max(region_start), region_size)),
    }
}

/// Returns a Vec of the valid memory addresses.
/// These should be used to configure the GuestMemoryMmap structure for the platform.
/// For x86_64 all addresses are valid from the start of the kernel except an 1GB
/// carve out at the end of 32bit address space and a second 256GB one at the 256GB limit.
pub fn arch_memory_regions(size: usize) -> Vec<(GuestAddress, usize)> {
    // If we get here with size == 0 something has seriously gone wrong. Firecracker should never
    // try to allocate guest memory of size 0
    assert!(size > 0, "Attempt to allocate guest memory of length 0");

    let dram_size = std::cmp::min(
        usize::MAX - u64_to_usize(MMIO32_MEM_SIZE) - u64_to_usize(MMIO64_MEM_SIZE),
        size,
    );

    if dram_size != size {
        warn!(
            "Requested memory size {} exceeds architectural maximum (1022GiB). Size has been \
             truncated to {}",
            size, dram_size
        );
    }

    let mut regions = vec![];

    if let Some((start_past_32bit_gap, remaining_past_32bit_gap)) = arch_memory_regions_with_gap(
        &mut regions,
        0,
        dram_size,
        u64_to_usize(MMIO32_MEM_START),
        u64_to_usize(MMIO32_MEM_SIZE),
    ) && let Some((start_past_64bit_gap, remaining_past_64bit_gap)) =
        arch_memory_regions_with_gap(
            &mut regions,
            start_past_32bit_gap,
            remaining_past_32bit_gap,
            u64_to_usize(MMIO64_MEM_START),
            u64_to_usize(MMIO64_MEM_SIZE),
        )
    {
        regions.push((
            GuestAddress(start_past_64bit_gap as u64),
            remaining_past_64bit_gap,
        ));
    }

    for (region_start, region_size) in &regions {
        warn!(
            "[LOG] Memory region: start_addr={:#x}, size={:#x} Bytes",
            region_start.raw_value(),
            region_size
        );
    }

    regions
}

/// Creates a `Vec` of `GuestRegionMmap` with the given configuration
pub fn create(
    regions: impl Iterator<Item = (GuestAddress, usize)>,
    mmap_flags: libc::c_int,
    eqdev_fd: Option<i32>,
) -> AxResult<Vec<GuestRegionMmap>> {
    let mut offset = 0;
    let file = eqdev_fd.map(|fd| Arc::new(unsafe { File::from_raw_fd(fd) }));
    regions
        .map(|(start, size)| {
            let mut builder = MmapRegionBuilder::new_with_bitmap(size, None)
                .with_mmap_prot(libc::PROT_READ | libc::PROT_WRITE)
                .with_mmap_flags(libc::MAP_NORESERVE | mmap_flags);

            if let Some(ref file) = file {
                let file_offset = FileOffset::from_arc(Arc::clone(file), offset);

                builder = builder.with_file_offset(file_offset);
            }

            offset = match offset.checked_add(size as u64) {
                None => return ax_err!(InvalidInput, "Offset overflow"),
                Some(new_off) if new_off >= i64::MAX as u64 => {
                    return ax_err!(InvalidInput, "Offset overflow");
                }
                Some(new_off) => new_off,
            };

            GuestRegionMmap::new(
                builder.build().map_err(|e| {
                    ax_err_type!(BadState, format_args!("Failed to create MmapRegion: {}", e))
                })?,
                start,
            )
            .ok_or(ax_err_type!(BadState, "Failed to create GuestRegionMmap"))
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Creates a GuestMemoryMmap from raw regions.
pub fn alloc_from_eqvisor(
    regions: impl Iterator<Item = (GuestAddress, usize)>,
    eqdev_fd: i32,
) -> AxResult<Vec<GuestRegionMmap>> {
    create(
        regions,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
        Some(eqdev_fd),
    )
}
