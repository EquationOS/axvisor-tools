use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;

use axerrno::{AxResult, ax_err, ax_err_type};

use crate::ioctl;
use crate::ioctl::EQINSTANCE_DEV_PREFIX;
use crate::microvm::config::{
    BootConfig, BootSource, BootSourceConfig, GuestConfig, IovaMode, MachineConfig, PciBdf,
    parse_iova_mode, parse_pci_bdf,
};
use crate::microvm::vstate::memory;
use crate::microvm::vstate::memory::{GuestAddress, GuestRegionMmap};
use crate::utils::mib_to_bytes;

/// A data structure that encapsulates the device configurations
/// held in the Vmm.
#[derive(Debug, Default)]
pub struct VmResources {
    pub vm_id: usize,
    /// The vCpu and memory configuration for this microVM.
    pub machine_config: MachineConfig,
    /// The boot source spec (contains both config and builder) for this microVM.
    pub boot_source: BootSource,
    /// The file descriptor of the instance device.
    pub fd: i32,
    /// Optional host PCI BDF list requested by the user for passthrough.
    pub passthrough_devices: Vec<PciBdf>,
    /// Optional VFIO settings tied to passthrough devices.
    pub vfio: Option<VfioResourceConfig>,
    /// GPA of the microVM PV console ring page.
    pub microvm_console_ring_gpa: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct VfioResourceConfig {
    pub iommu_group: u32,
    pub guest_visible_bdf: PciBdf,
    pub iova_mode: IovaMode,
    pub bars: [VfioBarInfo; 6],
    /// Snapshot of host PCI config space. This is the authoritative source used
    /// by axvisor for guest probe reads, so we do not depend on host CF8/CFC
    /// timing at runtime.
    pub pci_cfg_space_len: usize,
    pub pci_cfg_space: [u8; 256],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VfioBarInfo {
    pub start: u64,
    pub size: u64,
    pub flags: u64,
}

fn host_bdf_path(bdf: PciBdf) -> PathBuf {
    PathBuf::from(format!(
        "/sys/bus/pci/devices/{:04x}:{:02x}:{:02x}.{}",
        bdf.domain, bdf.bus, bdf.device, bdf.function
    ))
}

fn parse_iommu_group_id(dev_path: &PathBuf) -> AxResult<u32> {
    let group_link = fs::read_link(dev_path.join("iommu_group")).map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!("Failed to read iommu_group symlink: {}", e)
        )
    })?;
    let name = group_link
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ax_err_type!(InvalidInput, "Invalid iommu_group symlink target"))?;
    name.parse::<u32>().map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!("Invalid iommu_group id '{}': {}", name, e)
        )
    })
}

fn read_bound_driver(dev_path: &PathBuf) -> AxResult<String> {
    let driver_link = fs::read_link(dev_path.join("driver")).map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!("Failed to read driver symlink: {}", e)
        )
    })?;
    driver_link
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ax_err_type!(InvalidInput, "Invalid driver symlink target"))
}

fn parse_vfio_bars(dev_path: &PathBuf) -> AxResult<[VfioBarInfo; 6]> {
    fn parse_hex_u64(raw: &str, field: &str) -> AxResult<u64> {
        let normalized = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
        u64::from_str_radix(normalized, 16).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!("Invalid {} '{}': {}", field, raw, e)
            )
        })
    }

    let resource = fs::read_to_string(dev_path.join("resource")).map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!("Failed to read PCI resource file: {}", e)
        )
    })?;
    let mut bars = [VfioBarInfo::default(); 6];
    for (i, line) in resource.lines().take(6).enumerate() {
        let mut cols = line.split_whitespace();
        let start = cols
            .next()
            .ok_or_else(|| ax_err_type!(InvalidInput, "Malformed resource line: missing start"))?;
        let end = cols
            .next()
            .ok_or_else(|| ax_err_type!(InvalidInput, "Malformed resource line: missing end"))?;
        let flags = cols
            .next()
            .ok_or_else(|| ax_err_type!(InvalidInput, "Malformed resource line: missing flags"))?;
        // Sysfs resource lines are commonly printed as `0x... 0x... 0x...`.
        // Accept both with/without `0x` prefix to keep parser robust.
        let start = parse_hex_u64(start, "BAR start")?;
        let end = parse_hex_u64(end, "BAR end")?;
        let flags = parse_hex_u64(flags, "BAR flags")?;
        let size = if start == 0 || end < start {
            0
        } else {
            end - start + 1
        };
        bars[i] = VfioBarInfo { start, size, flags };
    }
    Ok(bars)
}

fn read_pci_cfg_space(dev_path: &PathBuf) -> AxResult<(usize, [u8; 256])> {
    fn read_prefixed_hex_u16(path: &PathBuf, field: &str) -> AxResult<u16> {
        let raw = fs::read_to_string(path).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!("Failed to read {} from {:?}: {}", field, path, e)
            )
        })?;
        let normalized = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
        u16::from_str_radix(normalized, 16).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!(
                    "Invalid {} value '{}' in {:?}: {}",
                    field,
                    raw.trim(),
                    path,
                    e
                )
            )
        })
    }

    fn read_prefixed_hex_u32(path: &PathBuf, field: &str) -> AxResult<u32> {
        let raw = fs::read_to_string(path).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!("Failed to read {} from {:?}: {}", field, path, e)
            )
        })?;
        let normalized = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
        u32::from_str_radix(normalized, 16).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!(
                    "Invalid {} value '{}' in {:?}: {}",
                    field,
                    raw.trim(),
                    path,
                    e
                )
            )
        })
    }

    // Linux exposes PCI config bytes through sysfs. Reading from this file is a
    // stable host-kernel path and does not rely on ad-hoc CF8/CFC port cycles.
    let raw = fs::read(dev_path.join("config")).map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!("Failed to read PCI config space from sysfs: {}", e)
        )
    })?;
    let mut cfg = [0u8; 256];
    let len = core::cmp::min(cfg.len(), raw.len());
    cfg[..len].copy_from_slice(&raw[..len]);

    // Some devices (notably certain SR-IOV VFs after vfio-pci bind) can return
    // 0xffff in Vendor/Device ID through `config` while still exposing valid
    // identity in sysfs scalar attributes. Patch the first dword so guest PCI
    // probe does not treat the function as absent.
    let vendor = u16::from_le_bytes([cfg[0], cfg[1]]);
    let device = u16::from_le_bytes([cfg[2], cfg[3]]);
    if vendor == 0xffff || device == 0xffff {
        let vendor = read_prefixed_hex_u16(&dev_path.join("vendor"), "vendor id")?;
        let device = read_prefixed_hex_u16(&dev_path.join("device"), "device id")?;
        cfg[0..2].copy_from_slice(&vendor.to_le_bytes());
        cfg[2..4].copy_from_slice(&device.to_le_bytes());

        // class file format is 0x00CCSSPP (class/subclass/prog-if).
        // We inject these bytes only when the snapshot does not already provide
        // sane values, preserving real config data whenever available.
        let class_triplet = read_prefixed_hex_u32(&dev_path.join("class"), "class code")?;
        if cfg[0x0b] == 0xff && cfg[0x0a] == 0xff && cfg[0x09] == 0xff {
            cfg[0x0b] = ((class_triplet >> 16) & 0xff) as u8;
            cfg[0x0a] = ((class_triplet >> 8) & 0xff) as u8;
            cfg[0x09] = (class_triplet & 0xff) as u8;
        }
        if cfg[0x0e] == 0xff {
            // Default to type-0 endpoint header when header type is unreadable.
            cfg[0x0e] = 0x00;
        }

        warn!(
            "PCI cfg snapshot vendor/device from config file is invalid, patched from sysfs attributes for {:?}: vendor={:04x} device={:04x}",
            dev_path, vendor, device
        );
    }
    Ok((len, cfg))
}

impl VmResources {
    /// Configures Vmm resources as described by the `config_json` param.
    pub fn from_json(config_json: &str) -> AxResult<Self> {
        let guest_config = serde_json::from_str::<GuestConfig>(&config_json).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!("Failed to parse guest configuration: {}", e)
            )
        })?;

        let machine_config = guest_config.machine_config.unwrap_or(MachineConfig {
            vcpu_count: 1,
            default_vcpu_num: None,
            max_vcpu_count: None,
            max_vcpu_num: None,
            init_mem_size_mib: 512,
            max_mem_size_mib: None,
        });

        let passthrough_devices = match guest_config.passthrough_devices {
            Some(devs) => {
                let mut normalized = Vec::with_capacity(devs.len());
                for dev in devs {
                    let bdf = parse_pci_bdf(&dev).ok_or_else(|| {
                        ax_err_type!(
                            InvalidInput,
                            format_args!(
                                "Invalid PCI BDF '{}', expected bb:dd.f or dddd:bb:dd.f",
                                dev
                            )
                        )
                    })?;
                    normalized.push(bdf);
                }
                normalized
            }
            None => Vec::new(),
        };
        for bdf in &passthrough_devices {
            let dev_path = host_bdf_path(*bdf);
            if !dev_path.exists() {
                return ax_err!(
                    InvalidInput,
                    format_args!(
                        "passthrough device {} not found on host: {:?}",
                        bdf.format(),
                        dev_path
                    )
                );
            }
            debug!("passthrough host device detected: {}", bdf.format());
        }

        let vfio_cfg_input = if guest_config.vfio.is_some() {
            guest_config.vfio
        } else if !passthrough_devices.is_empty() {
            // Route A requires host-side stable config snapshot. To keep the
            // user config simple, we auto-enable VFIO metadata mode whenever a
            // passthrough device is configured, even without an explicit
            // `vfio` section in microvm.json.
            Some(crate::microvm::config::VfioConfig {
                iommu_group: None,
                guest_visible_bdf: None,
                iova_mode: None,
            })
        } else {
            None
        };

        let vfio = match vfio_cfg_input {
            Some(vfio_cfg) => {
                let host_bdf = passthrough_devices.first().copied().ok_or_else(|| {
                    ax_err_type!(
                        InvalidInput,
                        "vfio metadata mode requires at least one passthrough-devices entry"
                    )
                })?;
                let dev_path = host_bdf_path(host_bdf);

                // Always trust host sysfs IOMMU topology:
                //   readlink /sys/bus/pci/devices/<BDF>/iommu_group -> <group-id>
                // `vfio.iommu-group` in JSON is treated as backward-compatible hint only.
                let actual_group = parse_iommu_group_id(&dev_path)?;
                if let Some(cfg_group) = vfio_cfg.iommu_group {
                    if cfg_group != actual_group {
                        warn!(
                            "Ignore vfio.iommu-group={} from config, using host sysfs group={} for {}",
                            cfg_group,
                            actual_group,
                            host_bdf.format()
                        );
                    }
                }
                let iommu_group = actual_group;
                let driver = read_bound_driver(&dev_path)?;
                const VFIO_DRIVER: &str = "vfio-pci";
                if driver != VFIO_DRIVER {
                    return ax_err!(
                        InvalidInput,
                        format_args!(
                            "Host PCI device must be bound to '{}', current driver is '{}'",
                            VFIO_DRIVER, driver
                        )
                    );
                }
                let bars = parse_vfio_bars(&dev_path)?;
                let (pci_cfg_space_len, pci_cfg_space) = read_pci_cfg_space(&dev_path)?;
                let guest_visible_bdf = match vfio_cfg.guest_visible_bdf {
                    Some(bdf) => parse_pci_bdf(&bdf).ok_or_else(|| {
                        ax_err_type!(
                            InvalidInput,
                            format_args!(
                                "Invalid vfio.guest-visible-bdf '{}', expected bb:dd.f or dddd:bb:dd.f",
                                bdf
                            )
                        )
                    })?,
                    None => PciBdf {
                        domain: 0,
                        bus: 0,
                        device: 0,
                        function: 0,
                    },
                };
                let iova_mode = match vfio_cfg.iova_mode {
                    Some(mode) => parse_iova_mode(&mode).ok_or_else(|| {
                        ax_err_type!(
                            InvalidInput,
                            format_args!(
                                "Invalid vfio.iova-mode '{}', only 'gpa-identity' is supported",
                                mode
                            )
                        )
                    })?,
                    None => IovaMode::GpaIdentity,
                };

                Some(VfioResourceConfig {
                    iommu_group,
                    guest_visible_bdf,
                    iova_mode,
                    bars,
                    pci_cfg_space_len,
                    pci_cfg_space,
                })
            }
            None => None,
        };

        // First, create the instance through ioctl, eqdriver will trigger the hvc to create the instance.
        let create_result = ioctl::ioctl_create_microvm(
            machine_config.default_vcpu_count(),
            machine_config.max_vcpu_count(),
            machine_config.init_mem_size_mib,
            machine_config.max_mem_size_mib(),
            &passthrough_devices,
            vfio,
        )
        .expect("Failed to create instance for dynamic loading");
        let microvm_id = create_result.instance_id;

        if let Some(vfio_cfg) = &vfio {
            info!(
                "VFIO precheck passed: host={} guest-visible={} iommu-group={} iova-mode={:?}",
                passthrough_devices
                    .first()
                    .copied()
                    .map(|bdf| bdf.format())
                    .unwrap_or_else(|| "n/a".to_string()),
                vfio_cfg.guest_visible_bdf.format(),
                vfio_cfg.iommu_group,
                vfio_cfg.iova_mode
            );
            for (i, bar) in vfio_cfg.bars.iter().enumerate() {
                if bar.size != 0 {
                    info!(
                        "VFIO BAR{} start={:#x} size={:#x} flags={:#x}",
                        i, bar.start, bar.size, bar.flags
                    );
                }
            }
            if vfio_cfg.pci_cfg_space_len >= 16 {
                let vendor =
                    u16::from_le_bytes([vfio_cfg.pci_cfg_space[0], vfio_cfg.pci_cfg_space[1]]);
                let device =
                    u16::from_le_bytes([vfio_cfg.pci_cfg_space[2], vfio_cfg.pci_cfg_space[3]]);
                let class_code = vfio_cfg.pci_cfg_space[0x0b];
                let subclass = vfio_cfg.pci_cfg_space[0x0a];
                info!(
                    "VFIO PCI cfg snapshot: len={} vendor={:04x} device={:04x} class={:02x}{:02x}",
                    vfio_cfg.pci_cfg_space_len, vendor, device, class_code, subclass
                );
            }
        }

        info!(
            "Create microVM instance success, instance ID = [{}]",
            microvm_id
        );

        let instance_dev_path_str = format!("{}{}\0", EQINSTANCE_DEV_PREFIX, microvm_id);
        let instance_dev_path = CStr::from_bytes_with_nul(instance_dev_path_str.as_bytes())
            .expect("Failed to create CStr for instance device path");

        let instance_fd = unsafe {
            libc::open(
                instance_dev_path.as_ptr() as *const libc::c_char,
                libc::O_RDWR,
            )
        };

        if instance_fd < 0 {
            error!(
                "Failed to open instance device {:?}: {}",
                instance_dev_path,
                std::io::Error::last_os_error()
            );
            return ax_err!(Io, "Failed to open instance device");
        }

        let mut resources: Self = Self {
            vm_id: microvm_id,
            machine_config,
            fd: instance_fd,
            passthrough_devices,
            vfio,
            microvm_console_ring_gpa: create_result.console_ring_gpa,
            ..Default::default()
        };

        resources.build_boot_source(guest_config.boot_source)?;

        Ok(resources)
    }

    /// Obtains the boot source hooks (kernel fd, command line creation and validation).
    pub fn build_boot_source(&mut self, boot_source_cfg: BootSourceConfig) -> AxResult {
        self.boot_source = BootSource {
            builder: Some(BootConfig::new(&boot_source_cfg)?),
            config: boot_source_cfg,
        };

        Ok(())
    }

    /// Allocates the given guest memory regions.
    ///
    /// If vhost-user-blk devices are in use, allocates memfd-backed shared memory, otherwise
    /// prefers anonymous memory for performance reasons.
    fn allocate_memory_regions(
        &self,
        regions: &[(GuestAddress, usize)],
    ) -> AxResult<Vec<GuestRegionMmap>> {
        memory::alloc_from_eqvisor(regions.iter().copied(), self.fd)
    }

    /// Allocates guest memory in a configuration most appropriate for these [`VmResources`].
    pub fn allocate_guest_memory(&self) -> AxResult<Vec<GuestRegionMmap>> {
        warn!(
            "[LOG] allocate_guest_memory init size {:#x} Bytes {} MBytes",
            mib_to_bytes(self.machine_config.init_mem_size_mib),
            self.machine_config.init_mem_size_mib
        );

        let regions =
            memory::arch_memory_regions(mib_to_bytes(self.machine_config.init_mem_size_mib));
        self.allocate_memory_regions(&regions)
    }
}
