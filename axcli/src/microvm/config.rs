//! Configuration module for axcli

use serde::{Deserialize, Serialize};

pub use boot_source::*;

/// Struct used in PUT `/machine-config` API call.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    /// Number of vcpu to start. Kept for backward-compatible configs.
    #[serde(default = "default_vcpu_count")]
    pub vcpu_count: u8,
    /// Preferred name for the default online vCPU count at boot.
    pub default_vcpu_num: Option<u8>,
    /// Maximum number of vcpu supported, if hotplug is supported.
    pub max_vcpu_count: Option<u8>,
    /// Preferred name for the maximum possible vCPU count.
    pub max_vcpu_num: Option<u8>,
    /// The size of the memory to allocate at startup, in MiB.
    pub init_mem_size_mib: usize,
    /// Maximum memory size in MiB, if memory hotplug is supported.
    pub max_mem_size_mib: Option<usize>,
}

const fn default_vcpu_count() -> u8 {
    1
}

impl MachineConfig {
    /// Get the default online vCPU count at boot.
    pub fn default_vcpu_count(&self) -> u8 {
        self.default_vcpu_num.unwrap_or(self.vcpu_count)
    }

    /// Get the maximum vCPU count, defaulting to `vcpu_count` if `max_vcpu_count` is not set.
    pub fn max_vcpu_count(&self) -> u8 {
        self.max_vcpu_num
            .or(self.max_vcpu_count)
            .unwrap_or_else(|| self.default_vcpu_count())
    }

    /// Get the maximum memory size in MiB, defaulting to `init_mem_size_mib` if
    /// `max_mem_size_mib` is not set.
    pub fn max_mem_size_mib(&self) -> usize {
        self.max_mem_size_mib.unwrap_or(self.init_mem_size_mib)
    }
}

/// Used for configuring a vmm from one single json passed to the Firecracker process.
#[derive(Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct GuestConfig {
    pub boot_source: BootSourceConfig,
    pub machine_config: Option<MachineConfig>,
    /// Optional host PCI devices to passthrough, identified by BDF.
    /// Supported forms:
    /// - "bb:dd.f"        (e.g. "15:00.0")
    /// - "dddd:bb:dd.f"   (e.g. "0000:15:00.0")
    pub passthrough_devices: Option<Vec<String>>,
    /// Optional VFIO-related passthrough settings.
    pub vfio: Option<VfioConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct VfioConfig {
    /// Host IOMMU group ID that owns the passthrough device.
    pub iommu_group: Option<u32>,
    /// Guest-visible BDF for the passthrough device.
    /// If absent, EqVisor defaults to `00:00.0`.
    pub guest_visible_bdf: Option<String>,
    /// IOVA mode for DMA mapping.
    /// Supported values:
    /// - "gpa-identity" (IOVA == GPA)
    pub iova_mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBdf {
    pub domain: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IovaMode {
    GpaIdentity,
}

impl PciBdf {
    /// Encode into a compact u64: 0xddddbbddf
    pub fn encode_u64(self) -> u64 {
        ((self.domain as u64) << 20)
            | ((self.bus as u64) << 12)
            | ((self.device as u64) << 4)
            | (self.function as u64)
    }

    pub fn format(self) -> String {
        format!(
            "{:04x}:{:02x}:{:02x}.{}",
            self.domain, self.bus, self.device, self.function
        )
    }
}

/// Normalize and validate host PCI BDF strings.
pub fn normalize_pci_bdf(input: &str) -> Option<String> {
    let bdf = input.trim().to_ascii_lowercase();
    if is_short_bdf(&bdf) || is_full_bdf(&bdf) {
        Some(bdf)
    } else {
        None
    }
}

pub fn parse_pci_bdf(input: &str) -> Option<PciBdf> {
    let bdf = normalize_pci_bdf(input)?;
    if is_short_bdf(&bdf) {
        // bb:dd.f
        let mut parts = bdf.split(':');
        let bus = u8::from_str_radix(parts.next()?, 16).ok()?;
        let mut df = parts.next()?.split('.');
        let dev = u8::from_str_radix(df.next()?, 16).ok()?;
        let func = u8::from_str_radix(df.next()?, 16).ok()?;
        Some(PciBdf {
            domain: 0,
            bus,
            device: dev,
            function: func,
        })
    } else if is_full_bdf(&bdf) {
        // dddd:bb:dd.f
        let mut parts = bdf.split(':');
        let domain = u16::from_str_radix(parts.next()?, 16).ok()?;
        let bus = u8::from_str_radix(parts.next()?, 16).ok()?;
        let mut df = parts.next()?.split('.');
        let dev = u8::from_str_radix(df.next()?, 16).ok()?;
        let func = u8::from_str_radix(df.next()?, 16).ok()?;
        Some(PciBdf {
            domain,
            bus,
            device: dev,
            function: func,
        })
    } else {
        None
    }
}

pub fn parse_iova_mode(input: &str) -> Option<IovaMode> {
    match input.trim().to_ascii_lowercase().as_str() {
        "gpa-identity" => Some(IovaMode::GpaIdentity),
        _ => None,
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn is_short_bdf(bdf: &str) -> bool {
    // bb:dd.f
    let mut parts = bdf.split(':');
    let bus = parts.next().unwrap_or_default();
    let dev_func = parts.next().unwrap_or_default();
    if parts.next().is_some() || bus.len() != 2 || !is_hex(bus) {
        return false;
    }
    let mut df = dev_func.split('.');
    let dev = df.next().unwrap_or_default();
    let func = df.next().unwrap_or_default();
    if df.next().is_some() || dev.len() != 2 || func.len() != 1 {
        return false;
    }
    is_hex(dev) && func.bytes().all(|c| (b'0'..=b'7').contains(&c))
}

fn is_full_bdf(bdf: &str) -> bool {
    // dddd:bb:dd.f
    let mut parts = bdf.split(':');
    let domain = parts.next().unwrap_or_default();
    let bus = parts.next().unwrap_or_default();
    let dev_func = parts.next().unwrap_or_default();
    if parts.next().is_some() || domain.len() != 4 || bus.len() != 2 {
        return false;
    }
    if !is_hex(domain) || !is_hex(bus) {
        return false;
    }
    let mut df = dev_func.split('.');
    let dev = df.next().unwrap_or_default();
    let func = df.next().unwrap_or_default();
    if df.next().is_some() || dev.len() != 2 || func.len() != 1 {
        return false;
    }
    is_hex(dev) && func.bytes().all(|c| (b'0'..=b'7').contains(&c))
}

mod boot_source {
    use std::fs::File;

    use serde::{Deserialize, Serialize};

    use axerrno::{AxResult, ax_err_type};

    /// Default guest kernel command line:
    /// - `reboot=k` shut down the guest on reboot, instead of well... rebooting;
    /// - `panic=1` on panic, reboot after 1 second;
    /// - `nomodule` disable loadable kernel module support;
    /// - `8250.nr_uarts=0` disable 8250 serial interface;
    /// - `i8042.noaux` do not probe the i8042 controller for an attached mouse (save boot time);
    /// - `i8042.nomux` do not probe i8042 for a multiplexing controller (save boot time);
    /// - `i8042.dumbkbd` do not attempt to control kbd state via the i8042 (save boot time).
    /// - `swiotlb=noforce` disable software bounce buffers (SWIOTLB)
    pub const DEFAULT_KERNEL_CMDLINE: &str = "reboot=k panic=1 nomodule 8250.nr_uarts=0 i8042.noaux \
                                          i8042.nomux i8042.dumbkbd swiotlb=noforce";

    /// Kernel command line maximum size.
    pub const CMDLINE_MAX_SIZE: usize = 2048;

    /// Strongly typed data structure used to configure the boot source of the
    /// microvm.
    #[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct BootSourceConfig {
        /// Path of the kernel image.
        pub kernel_image_path: String,
        /// Path of the initrd, if there is one.
        pub initrd_path: Option<String>,
        /// The boot arguments to pass to the kernel. If this field is uninitialized,
        /// DEFAULT_KERNEL_CMDLINE is used.
        pub boot_args: Option<String>,
    }

    /// Holds the kernel specification (both configuration as well as runtime details).
    #[derive(Debug, Default)]
    pub struct BootSource {
        /// The boot source configuration.
        #[allow(unused)]
        pub config: BootSourceConfig,
        /// The boot source builder (a boot source allocated and validated).
        /// It is an option cause a resumed microVM does not need it.
        pub builder: Option<BootConfig>,
    }

    /// Holds the kernel builder (created and validates based on BootSourceConfig).
    #[derive(Debug)]
    pub struct BootConfig {
        /// The commandline validated against correctness.
        pub cmdline: linux_loader::cmdline::Cmdline,
        /// The descriptor to the kernel file.
        pub kernel_file: File,
        /// The descriptor to the initrd file, if there is one.
        pub initrd_file: Option<File>,
    }

    impl BootConfig {
        /// Creates the BootConfig based on a given configuration.
        pub fn new(cfg: &BootSourceConfig) -> AxResult<Self> {
            // Validate boot source config.
            let kernel_file = File::open(&cfg.kernel_image_path).map_err(|e| {
                ax_err_type!(
                    InvalidInput,
                    format_args!("Invalid kernel_image_path: {}", e)
                )
            })?;
            let initrd_file: Option<File> = match &cfg.initrd_path {
                Some(path) => Some(File::open(path).map_err(|e| {
                    ax_err_type!(
                        InvalidInput,
                        format_args!("Invalid failed to open initrd_path: {}", e)
                    )
                })?),
                None => None,
            };

            let cmdline_str = match cfg.boot_args.as_ref() {
                None => DEFAULT_KERNEL_CMDLINE,
                Some(str) => str.as_str(),
            };
            let cmdline = linux_loader::cmdline::Cmdline::try_from(cmdline_str, CMDLINE_MAX_SIZE)
                .map_err(|err| {
                ax_err_type!(
                    InvalidInput,
                    format_args!("Invalid kernel command line: {}", err)
                )
            })?;

            Ok(BootConfig {
                cmdline,
                kernel_file,
                initrd_file,
            })
        }
    }
}
