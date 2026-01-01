// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axerrno::{AxResult, ax_err_type};
use serde::{Deserialize, Serialize};

use crate::microvm::memory::{
    GuestMemory, GuestMemoryMmap, GuestMemoryRegion, GuestRegionMmap, GuestRegionMmapExt,
};

/// Architecture independent parts of a VM.
#[derive(Debug)]
pub struct Vm {
    /// The file descriptor used to access this Vm.
    pub fd: i32,
    /// The guest memory of this Vm.
    pub guest_memory: GuestMemoryMmap,
}

/// Contains Vm functions that are usable across CPU architectures
impl Vm {
    pub fn new(fd: i32) -> AxResult<Vm> {
        Ok(Vm {
            fd,
            guest_memory: GuestMemoryMmap::default(),
        })
    }

    fn register_memory_region(&mut self, region: Arc<GuestRegionMmapExt>) -> AxResult {
        let new_guest_memory = self
            .guest_memory
            .insert_region(Arc::clone(&region))
            .map_err(|e| {
                ax_err_type!(
                    BadState,
                    format_args!("Failed to register memory region: {}", e)
                )
            })?;

        self.guest_memory = new_guest_memory;

        Ok(())
    }

    /// Register a list of new memory regions to this [`Vm`].
    pub fn register_dram_memory_regions(&mut self, regions: Vec<GuestRegionMmap>) -> AxResult {
        for region in regions {
            let arcd_region = Arc::new(GuestRegionMmapExt::dram_from_mmap_region(region));

            self.register_memory_region(arcd_region)?
        }

        Ok(())
    }

    /// Gets a reference to the kvm file descriptor owned by this VM.
    pub fn fd(&self) -> i32 {
        self.fd
    }

    /// Gets a reference to this [`Vm`]'s [`GuestMemoryMmap`] object
    pub fn guest_memory(&self) -> &GuestMemoryMmap {
        &self.guest_memory
    }
}
