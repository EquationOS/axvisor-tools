use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;

use axerrno::{AxResult, ax_err, ax_err_type};
use eqvm_defs::{MICROVM_BLOCK_FLAG_ENABLED, MICROVM_BLOCK_FLAG_READ_ONLY};

use crate::ioctl;
use crate::ioctl::EQINSTANCE_DEV_PREFIX;
use crate::microvm::config::{
    BlockDeviceConfig, BootConfig, BootSource, BootSourceConfig, DEFAULT_KERNEL_CMDLINE,
    GuestConfig, IovaMode, MachineConfig, PciBdf, parse_iova_mode, parse_pci_bdf,
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
    /// Host HPA of the split virtio-blk notify ring page.
    pub microvm_block_notify_ring_gpa: usize,
    /// Optional split virtio-blk drives served by axcli.
    pub block_devices: Vec<BlockDeviceConfig>,
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

fn validate_block_devices(
    drives: Option<Vec<BlockDeviceConfig>>,
) -> AxResult<Vec<BlockDeviceConfig>> {
    let drives = drives.unwrap_or_default();
    if drives.len() > 1 {
        return ax_err!(
            InvalidInput,
            "only one split virtio-blk drive is supported in this stage"
        );
    }
    let mut root_count = 0usize;
    let mut drive_ids = BTreeSet::new();

    for drive in &drives {
        let drive_id = drive.drive_id.trim();
        if drive_id.is_empty() {
            return ax_err!(InvalidInput, "drive_id must not be empty");
        }
        if !drive_ids.insert(drive_id.to_string()) {
            return ax_err!(
                InvalidInput,
                format_args!("duplicated drive_id '{}'", drive.drive_id)
            );
        }
        if drive.is_root_device {
            root_count += 1;
        }

        let metadata = fs::metadata(&drive.path_on_host).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!(
                    "Invalid drive path_on_host '{}' for drive '{}': {}",
                    drive.path_on_host, drive.drive_id, e
                )
            )
        })?;
        if !metadata.is_file() {
            return ax_err!(
                InvalidInput,
                format_args!(
                    "drive '{}' path_on_host must be a regular file: {}",
                    drive.drive_id, drive.path_on_host
                )
            );
        }
        let image_len = metadata.len();
        if image_len == 0 {
            return ax_err!(
                InvalidInput,
                format_args!(
                    "drive '{}' path_on_host must not be empty: {}",
                    drive.drive_id, drive.path_on_host
                )
            );
        }
        if image_len % 512 != 0 {
            return ax_err!(
                InvalidInput,
                format_args!(
                    "drive '{}' path_on_host size must be 512-byte aligned: {} bytes ({})",
                    drive.drive_id, image_len, drive.path_on_host
                )
            );
        }
        if let Some(cache_type) = &drive.cache_type {
            warn!(
                "drive '{}' cache_type='{}' parsed but not enforced until virtio-blk data path is enabled",
                drive.drive_id, cache_type
            );
        }
        if let Some(io_engine) = &drive.io_engine {
            warn!(
                "drive '{}' io_engine='{}' parsed but not enforced until virtio-blk data path is enabled",
                drive.drive_id, io_engine
            );
        }
    }

    if root_count > 1 {
        return ax_err!(InvalidInput, "only one root block drive is supported");
    }

    Ok(drives)
}

fn validate_machine_config(machine_config: &MachineConfig) -> AxResult {
    let default_vcpus = machine_config.default_vcpu_count();
    let max_vcpus = machine_config.max_vcpu_count();
    if default_vcpus == 0 {
        return ax_err!(InvalidInput, "default vCPU count must be at least 1");
    }
    if max_vcpus == 0 {
        return ax_err!(InvalidInput, "max vCPU count must be at least 1");
    }
    if default_vcpus > max_vcpus {
        return ax_err!(
            InvalidInput,
            format_args!(
                "default vCPU count {} must not exceed max vCPU count {}",
                default_vcpus, max_vcpus
            )
        );
    }

    Ok(())
}

fn block_metadata(drives: &[BlockDeviceConfig]) -> AxResult<(u64, u64)> {
    let Some(drive) = drives.first() else {
        return Ok((0, 0));
    };
    let metadata = fs::metadata(&drive.path_on_host).map_err(|e| {
        ax_err_type!(
            InvalidInput,
            format_args!(
                "Invalid drive path_on_host '{}' for drive '{}': {}",
                drive.path_on_host, drive.drive_id, e
            )
        )
    })?;
    let mut flags = MICROVM_BLOCK_FLAG_ENABLED as u64;
    if drive.is_read_only {
        flags |= MICROVM_BLOCK_FLAG_READ_ONLY as u64;
    }
    Ok((flags, metadata.len() / 512))
}

fn cmdline_has_key(cmdline: &str, key: &str) -> bool {
    cmdline.split_whitespace().any(|token| {
        token == key
            || token
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with('='))
    })
}

fn cmdline_has_token(cmdline: &str, token: &str) -> bool {
    cmdline.split_whitespace().any(|entry| entry == token)
}

fn ensure_rootfs_cmdline(boot_source_cfg: &mut BootSourceConfig) {
    let cmdline = boot_source_cfg
        .boot_args
        .get_or_insert_with(|| DEFAULT_KERNEL_CMDLINE.to_string());
    if cmdline_has_key(cmdline, "root") {
        info!("root block drive configured, preserving user supplied kernel root= argument");
        return;
    }

    if !cmdline.is_empty() && !cmdline.ends_with(' ') {
        cmdline.push(' ');
    }
    cmdline.push_str("root=/dev/vda");
    if !cmdline_has_token(cmdline, "rw") && !cmdline_has_token(cmdline, "ro") {
        cmdline.push_str(" rw");
    }
    if !cmdline_has_token(cmdline, "rootwait") {
        cmdline.push_str(" rootwait");
    }
    info!("root block drive configured, appended root=/dev/vda rw rootwait");
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
        validate_machine_config(&machine_config)?;
        let block_devices = validate_block_devices(guest_config.drives)?;
        let has_root_block_device = block_devices.iter().any(|drive| drive.is_root_device);

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
        let (block_flags, block_capacity_sectors) = block_metadata(&block_devices)?;
        let create_result = ioctl::ioctl_create_microvm(
            machine_config.default_vcpu_count(),
            machine_config.max_vcpu_count(),
            machine_config.init_mem_size_mib,
            machine_config.max_mem_size_mib(),
            &passthrough_devices,
            vfio,
            block_devices.len(),
            block_flags,
            block_capacity_sectors,
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
            microvm_block_notify_ring_gpa: create_result.block_notify_ring_gpa,
            block_devices,
            ..Default::default()
        };

        let mut boot_source_cfg = guest_config.boot_source;
        if has_root_block_device {
            ensure_rootfs_cmdline(&mut boot_source_cfg);
        }
        resources.build_boot_source(boot_source_cfg)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn block_drive(path: String, root: bool) -> BlockDeviceConfig {
        BlockDeviceConfig {
            drive_id: if root { "rootfs" } else { "data" }.to_string(),
            path_on_host: path,
            is_root_device: root,
            is_read_only: false,
            cache_type: None,
            io_engine: None,
        }
    }

    fn temp_block_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        path.push(format!(
            "eqvisor-axcli-block-test-{}-{}-{}",
            name,
            std::process::id(),
            suffix
        ));
        path
    }

    #[test]
    fn rootfs_cmdline_appends_default_root_args() {
        let mut cfg = BootSourceConfig {
            kernel_image_path: "/tmp/vmlinux".to_string(),
            initrd_path: None,
            boot_args: Some("console=hvc0 panic=1".to_string()),
        };

        ensure_rootfs_cmdline(&mut cfg);

        let cmdline = cfg.boot_args.expect("boot args");
        assert!(cmdline.contains("root=/dev/vda"));
        assert!(cmdline.contains(" rw"));
        assert!(cmdline.contains("rootwait"));
    }

    #[test]
    fn rootfs_cmdline_preserves_existing_root_arg() {
        let mut cfg = BootSourceConfig {
            kernel_image_path: "/tmp/vmlinux".to_string(),
            initrd_path: None,
            boot_args: Some("console=hvc0 root=/dev/vdb ro".to_string()),
        };

        ensure_rootfs_cmdline(&mut cfg);

        assert_eq!(
            cfg.boot_args.as_deref(),
            Some("console=hvc0 root=/dev/vdb ro")
        );
    }

    #[test]
    fn block_device_validation_rejects_multiple_roots() {
        let path = temp_block_path("multiple-roots");
        std::fs::write(&path, vec![0u8; 512]).expect("create temp backing file");

        let first = block_drive(path.to_string_lossy().to_string(), true);
        let second = BlockDeviceConfig {
            drive_id: "rootfs2".to_string(),
            ..first.clone()
        };

        let result = validate_block_devices(Some(vec![first, second]));
        let _ = std::fs::remove_file(&path);

        assert!(result.is_err());
    }

    #[test]
    fn block_device_validation_rejects_empty_backing_file() {
        let path = temp_block_path("empty");
        std::fs::write(&path, b"").expect("create temp backing file");

        let result = validate_block_devices(Some(vec![block_drive(
            path.to_string_lossy().to_string(),
            true,
        )]));
        let _ = std::fs::remove_file(&path);

        assert!(result.is_err());
    }

    #[test]
    fn block_device_validation_rejects_unaligned_backing_file() {
        let path = temp_block_path("unaligned");
        std::fs::write(&path, vec![0u8; 513]).expect("create temp backing file");

        let result = validate_block_devices(Some(vec![block_drive(
            path.to_string_lossy().to_string(),
            true,
        )]));
        let _ = std::fs::remove_file(&path);

        assert!(result.is_err());
    }

    #[test]
    fn machine_config_rejects_default_vcpus_above_max() {
        let machine_config = MachineConfig {
            vcpu_count: 2,
            default_vcpu_num: None,
            max_vcpu_count: Some(1),
            max_vcpu_num: None,
            init_mem_size_mib: 512,
            max_mem_size_mib: None,
        };

        assert!(validate_machine_config(&machine_config).is_err());
    }
}
