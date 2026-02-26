// Copyright © 2020, Oracle and/or its affiliates.
// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::mem;

use kvm_bindings::kvm_sregs;

use eqgate_microvm::layout::{PDE_START, PDPTE_START, PML4_START};

use crate::microvm::arch::BootProtocol;
use crate::microvm::arch::gdt::{gdt_entry, kvm_segment_from_gdt};
use crate::microvm::vstate::memory::{Address, Bytes, GuestAddress, GuestMemory, GuestMemoryMmap};

/// Errors thrown while setting up x86_64 registers.
#[derive(Debug, thiserror::Error, displaydoc::Display, PartialEq, Eq)]
pub enum RegsError {
    /// Writing the GDT to RAM failed.
    WriteGDT,
    /// Writing the IDT to RAM failed
    WriteIDT,
    /// WritePDPTEAddress
    WritePDPTEAddress,
    /// WritePDEAddress
    WritePDEAddress,
    /// WritePML4Address
    WritePML4Address,
}

/// Error type for [`setup_sregs`].
#[derive(Debug, thiserror::Error, displaydoc::Display, PartialEq, Eq)]
pub enum SetupSpecialRegistersError {
    // /// Failed to get special registers: {0}
    // GetSpecialRegisters(vmm_sys_util::errno::Error),
    /// Failed to configure segments and special registers: {0}
    ConfigureSegmentsAndSpecialRegisters(RegsError),
    /// Failed to setup page tables: {0}
    SetupPageTables(RegsError),
    // /// Failed to set special registers: {0}
    // SetSpecialRegisters(vmm_sys_util::errno::Error),
}

/// Configures the special registers and system page tables for a given CPU.
///
/// # Arguments
///
/// * `mem` - The memory that will be passed to the guest.
/// * `vcpu` - Structure for the VCPU that holds the VCPU's fd.
/// * `boot_prot` - The boot protocol being used.
///
/// # Errors
///
/// When:
/// - [`kvm_ioctls::ioctls::vcpu::VcpuFd::get_sregs`] errors.
/// - [`configure_segments_and_sregs`] errors.
/// - [`setup_page_tables`] errors
/// - [`kvm_ioctls::ioctls::vcpu::VcpuFd::set_sregs`] errors.
pub fn setup_sregs(
    mem: &GuestMemoryMmap,
    boot_prot: BootProtocol,
) -> Result<(), SetupSpecialRegistersError> {
    let mut sregs: kvm_sregs = Default::default();

    configure_segments_and_sregs(mem, &mut sregs, boot_prot)
        .map_err(SetupSpecialRegistersError::ConfigureSegmentsAndSpecialRegisters)?;
    if let BootProtocol::LinuxBoot = boot_prot {
        setup_page_tables(mem, &mut sregs).map_err(SetupSpecialRegistersError::SetupPageTables)?;
    }

    // vcpu.set_sregs(&sregs)
    //     .map_err(SetupSpecialRegistersError::SetSpecialRegisters)
    Ok(())
}

const BOOT_GDT_OFFSET: u64 = 0x500;
const BOOT_IDT_OFFSET: u64 = 0x520;

const BOOT_GDT_MAX: usize = 4;

const EFER_LMA: u64 = 0x400;
const EFER_LME: u64 = 0x100;

const X86_CR0_PE: u64 = 0x1;
const X86_CR0_ET: u64 = 0x10;
const X86_CR0_PG: u64 = 0x8000_0000;
const X86_CR4_PAE: u64 = 0x20;

fn write_gdt_table(table: &[u64], guest_mem: &GuestMemoryMmap) -> Result<(), RegsError> {
    let boot_gdt_addr = GuestAddress(BOOT_GDT_OFFSET);
    for (index, entry) in table.iter().enumerate() {
        let addr = guest_mem
            .checked_offset(boot_gdt_addr, index * mem::size_of::<u64>())
            .ok_or(RegsError::WriteGDT)?;
        guest_mem
            .write_obj(*entry, addr)
            .map_err(|_| RegsError::WriteGDT)?;
    }
    Ok(())
}

fn write_idt_value(val: u64, guest_mem: &GuestMemoryMmap) -> Result<(), RegsError> {
    let boot_idt_addr = GuestAddress(BOOT_IDT_OFFSET);
    guest_mem
        .write_obj(val, boot_idt_addr)
        .map_err(|_| RegsError::WriteIDT)
}

fn configure_segments_and_sregs(
    mem: &GuestMemoryMmap,
    sregs: &mut kvm_sregs,
    boot_prot: BootProtocol,
) -> Result<(), RegsError> {
    let gdt_table: [u64; BOOT_GDT_MAX] = match boot_prot {
        BootProtocol::PvhBoot => {
            // Configure GDT entries as specified by PVH boot protocol
            [
                gdt_entry(0, 0, 0),                // NULL
                gdt_entry(0xc09b, 0, 0xffff_ffff), // CODE
                gdt_entry(0xc093, 0, 0xffff_ffff), // DATA
                gdt_entry(0x008b, 0, 0x67),        // TSS
            ]
        }
        BootProtocol::LinuxBoot => {
            // Configure GDT entries as specified by Linux 64bit boot protocol
            [
                gdt_entry(0, 0, 0),            // NULL
                gdt_entry(0xa09b, 0, 0xfffff), // CODE
                gdt_entry(0xc093, 0, 0xfffff), // DATA
                gdt_entry(0x808b, 0, 0xfffff), // TSS
            ]
        }
    };

    let code_seg = kvm_segment_from_gdt(gdt_table[1], 1);
    let data_seg = kvm_segment_from_gdt(gdt_table[2], 2);
    let tss_seg = kvm_segment_from_gdt(gdt_table[3], 3);

    // Write segments
    write_gdt_table(&gdt_table[..], mem)?;
    sregs.gdt.base = BOOT_GDT_OFFSET;
    sregs.gdt.limit = u16::try_from(mem::size_of_val(&gdt_table)).unwrap() - 1;

    write_idt_value(0, mem)?;
    sregs.idt.base = BOOT_IDT_OFFSET;
    sregs.idt.limit = u16::try_from(mem::size_of::<u64>()).unwrap() - 1;

    sregs.cs = code_seg;
    sregs.ds = data_seg;
    sregs.es = data_seg;
    sregs.fs = data_seg;
    sregs.gs = data_seg;
    sregs.ss = data_seg;
    sregs.tr = tss_seg;

    match boot_prot {
        BootProtocol::PvhBoot => {
            sregs.cr0 = X86_CR0_PE | X86_CR0_ET;
            sregs.cr4 = 0;
        }
        BootProtocol::LinuxBoot => {
            // 64-bit protected mode
            sregs.cr0 |= X86_CR0_PE;
            sregs.efer |= EFER_LME | EFER_LMA;
        }
    }

    Ok(())
}

fn setup_page_tables(mem: &GuestMemoryMmap, sregs: &mut kvm_sregs) -> Result<(), RegsError> {
    // Puts PML4 right after zero page but aligned to 4k.
    let boot_pml4_addr = GuestAddress(PML4_START);
    let boot_pdpte_addr = GuestAddress(PDPTE_START);
    let boot_pde_addr = GuestAddress(PDE_START);

    // Entry covering VA [0..512GB)
    mem.write_obj(boot_pdpte_addr.raw_value() | 0x03, boot_pml4_addr)
        .map_err(|_| RegsError::WritePML4Address)?;

    // Entry covering VA [0..1GB)
    mem.write_obj(boot_pde_addr.raw_value() | 0x03, boot_pdpte_addr)
        .map_err(|_| RegsError::WritePDPTEAddress)?;
    // 512 2MB entries together covering VA [0..1GB). Note we are assuming
    // CPU supports 2MB pages (/proc/cpuinfo has 'pse'). All modern CPUs do.
    for i in 0..512 {
        mem.write_obj((i << 21) + 0x83u64, boot_pde_addr.unchecked_add(i * 8))
            .map_err(|_| RegsError::WritePDEAddress)?;
    }

    sregs.cr3 = boot_pml4_addr.raw_value();
    sregs.cr4 |= X86_CR4_PAE;
    sregs.cr0 |= X86_CR0_PG;
    Ok(())
}
