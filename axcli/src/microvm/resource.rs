use std::ffi::CStr;

use axerrno::{AxResult, ax_err, ax_err_type};

use crate::instance::EQINSTANCE_DEV_PREFIX;
use crate::ioctl;
use crate::microvm::config::{
    BootConfig, BootSource, BootSourceConfig, GuestConfig, MachineConfig,
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
            max_vcpu_count: None,
            init_mem_size_mib: 512,
            max_mem_size_mib: None,
        });

        // First, create the instance through ioctl, eqdriver will trigger the hvc to create the instance.
        let microvm_id = ioctl::ioctl_create_microvm(
            machine_config.vcpu_count,
            machine_config.max_vcpu_count(),
            machine_config.init_mem_size_mib,
            machine_config.max_mem_size_mib(),
        )
        .expect("Failed to create instance for dynamic loading");

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
