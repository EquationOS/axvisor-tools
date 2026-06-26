// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
#![cfg(target_arch = "x86_64")]

use std::fmt::Debug;
// use std::sync::{Arc, Mutex};

use acpi_tables::aml::AmlError;
use acpi_tables::{Aml, aml};
// use libc::EFD_NONBLOCK;
// use vm_superio::Serial;
// use vmm_sys_util::eventfd::EventFd;

// use crate::Vm;
// use crate::devices::legacy::serial::SerialOut;
// use crate::devices::legacy::{EventFdTrigger, I8042Device, SerialDevice, SerialEventsWrapper};
// use crate::vstate::bus::BusError;

// /// Errors corresponding to the `PortIODeviceManager`.
// #[derive(Debug, derive_more::From, thiserror::Error, displaydoc::Display)]
// pub enum LegacyDeviceError {
//     /// Failed to add legacy device to Bus: {0}
//     BusError(BusError),
//     /// Failed to create EventFd: {0}
//     EventFd(std::io::Error),
// }

/// The `PortIODeviceManager` is a wrapper that is used for registering legacy devices
/// on an I/O Bus. It currently manages the uart and i8042 devices.
/// The `LegacyDeviceManger` should be initialized only by using the constructor.
#[derive(Debug)]
pub struct PortIODeviceManager {
    // // BusDevice::Serial
    // pub stdio_serial: Arc<Mutex<SerialDevice>>,
    // // BusDevice::I8042Device
    // pub i8042: Arc<Mutex<I8042Device>>,

    // // Communication event on ports 1 & 3.
    // pub com_evt_1_3: EventFdTrigger,
    // // Communication event on ports 2 & 4.
    // pub com_evt_2_4: EventFdTrigger,
    // // Keyboard event.
    // pub kbd_evt: EventFd,
}

#[allow(dead_code)]
impl PortIODeviceManager {
    /// x86 global system interrupt for communication events on serial ports 1
    /// & 3. See
    /// <https://en.wikipedia.org/wiki/Interrupt_request_(PC_architecture)>.
    const COM_EVT_1_3_GSI: u32 = 4;
    /// x86 global system interrupt for communication events on serial ports 2
    /// & 4. See
    /// <https://en.wikipedia.org/wiki/Interrupt_request_(PC_architecture)>.
    const COM_EVT_2_4_GSI: u32 = 3;
    /// x86 global system interrupt for keyboard port.
    /// See <https://en.wikipedia.org/wiki/Interrupt_request_(PC_architecture)>.
    const KBD_EVT_GSI: u32 = 1;
    /// Legacy serial port device addresses. See
    /// <https://tldp.org/HOWTO/Serial-HOWTO-10.html#ss10.1>.
    const SERIAL_PORT_ADDRESSES: [u64; 4] = [0x3f8, 0x2f8, 0x3e8, 0x2e8];
    /// Size of legacy serial ports.
    const SERIAL_PORT_SIZE: u64 = 0x8;
    /// i8042 keyboard data register address. See
    /// <https://elixir.bootlin.com/linux/latest/source/drivers/input/serio/i8042-io.h#L41>.
    const I8042_KDB_DATA_REGISTER_ADDRESS: u64 = 0x060;
    /// i8042 keyboard data register size.
    const I8042_KDB_DATA_REGISTER_SIZE: u64 = 0x5;

    pub(crate) fn append_aml_bytes(bytes: &mut Vec<u8>) -> Result<(), AmlError> {
        // Set up COM devices
        let gsi = [
            Self::COM_EVT_1_3_GSI,
            Self::COM_EVT_2_4_GSI,
            Self::COM_EVT_1_3_GSI,
            Self::COM_EVT_2_4_GSI,
        ];
        for com in 0u8..4 {
            // COM1
            aml::Device::new(
                format!("_SB_.COM{}", com + 1).as_str().try_into()?,
                vec![
                    &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0501")?)?,
                    &aml::Name::new("_UID".try_into()?, &com)?,
                    &aml::Name::new("_DDN".try_into()?, &format!("COM{}", com + 1))?,
                    &aml::Name::new(
                        "_CRS".try_into().unwrap(),
                        &aml::ResourceTemplate::new(vec![
                            &aml::Interrupt::new(true, true, false, false, gsi[com as usize]),
                            &aml::Io::new(
                                PortIODeviceManager::SERIAL_PORT_ADDRESSES[com as usize]
                                    .try_into()
                                    .unwrap(),
                                PortIODeviceManager::SERIAL_PORT_ADDRESSES[com as usize]
                                    .try_into()
                                    .unwrap(),
                                1,
                                PortIODeviceManager::SERIAL_PORT_SIZE.try_into().unwrap(),
                            ),
                        ]),
                    )?,
                ],
            )
            .append_aml_bytes(bytes)?;
        }
        // Setup i8042
        aml::Device::new(
            "_SB_.PS2_".try_into()?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0303")?)?,
                &aml::Method::new(
                    "_STA".try_into()?,
                    0,
                    false,
                    vec![&aml::Return::new(&0x0fu8)],
                ),
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![
                        &aml::Io::new(
                            PortIODeviceManager::I8042_KDB_DATA_REGISTER_ADDRESS
                                .try_into()
                                .unwrap(),
                            PortIODeviceManager::I8042_KDB_DATA_REGISTER_ADDRESS
                                .try_into()
                                .unwrap(),
                            1u8,
                            1u8,
                        ),
                        // Fake a command port so Linux stops complaining
                        &aml::Io::new(0x0064, 0x0064, 1u8, 1u8),
                        &aml::Interrupt::new(true, true, false, false, Self::KBD_EVT_GSI),
                    ]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)
    }
}
