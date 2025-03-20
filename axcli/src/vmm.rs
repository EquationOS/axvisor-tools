use core::ffi::c_void;
use std::fs::read_to_string;
use std::fs::File;
use std::io::prelude::*;

use serde::Deserialize;
use serde::Serialize;

use crate::{VmBootShutdownArgs, VmCreateArgs};
use rustix::fd::OwnedFd;
use rustix::fs::{open, Mode, OFlags};
use rustix::ioctl;

#[derive(Debug, Default, Serialize, Deserialize)]
struct VmCreateCliArg {
    // Basic Information
    pub id: usize,
    pub name: String,
    pub vm_type: usize,
    // Resources.
    pub cpu_set: usize,

    pub entry_point: usize,

    pub bios_path: String,
    pub bios_load_addr: usize,

    pub kernel_path: String,
    pub kernel_load_addr: usize,

    pub ramdisk_path: Option<String>,
    pub ramdisk_load_addr: Option<usize>,

    pub disk_path: Option<String>,

    /// Memory Information
    pub memory_regions: Vec<VmMemCfg>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct VmMemCfg {
    pub gpa: usize,
    pub size: usize,
    pub flags: usize,
}

#[derive(Debug, Default)]
#[repr(C)]
struct VmCreateArg {
    pub id: usize,
    pub vm_name_ptr: usize,
    pub vm_name_len: usize,
    pub vm_type: usize,
    pub cpu_set: usize,
    pub bios_load_addr: usize,
    pub bios_img_size: usize,
    pub kernel_load_addr: usize,
    pub kernel_img_size: usize,
    pub ramdisk_load_addr: usize,
    pub ramdisk_img_size: usize,
}

unsafe impl ioctl::Ioctl for VmCreateArg {
    type Output = ();

    const OPCODE: ioctl::Opcode = ioctl::Opcode::write::<Self>(0, 7);

    const IS_MUTATING: bool = false;

    fn as_ptr(&mut self) -> *mut c_void {
        self as *const _ as *mut c_void
    }

    unsafe fn output_from_ptr(
        _out: ioctl::IoctlOutput,
        _extract_output: *mut c_void,
    ) -> rustix::io::Result<Self::Output> {
        Ok(())
    }
}

pub fn open_driver() -> OwnedFd {
    let driver = String::from("/dev/jailhouse");
    open(driver, OFlags::RDWR, Mode::RWXO).expect("Failed to open ArceOS driver")
}

pub fn axvmm_create_vm(arg: VmCreateArgs) -> Result<(), String> {
    let config_content = read_to_string(arg.config_path).map_err(|err| err.to_string())?;

    let vm_arg: VmCreateCliArg =
        toml::from_str(config_content.as_str()).map_err(|err| err.to_string())?;

    // let memory_regions_serialized = toml::to_string(vm_arg.memory_regions.as_slice()).unwrap();

    // debug!("memory_regions_serialized {}", memory_regions_serialized);

    debug!("get vm_arg {:#x?}", vm_arg);

    let bios_img = File::open(vm_arg.bios_path).unwrap();
    let kernel_img = File::open(vm_arg.kernel_path).unwrap();

    let ramdisk_img = vm_arg
        .ramdisk_path
        .map(|randisk_path| File::open(randisk_path).unwrap());

    let driver_arg = VmCreateArg {
        id: vm_arg.id,
        vm_name_ptr: vm_arg.name.as_ptr() as usize,
        vm_name_len: vm_arg.name.len(),
        vm_type: vm_arg.vm_type,
        cpu_set: vm_arg.cpu_set,
        bios_load_addr: vm_arg.bios_load_addr,
        bios_img_size: bios_img.metadata().unwrap().len() as usize,
        kernel_load_addr: vm_arg.kernel_load_addr,
        kernel_img_size: kernel_img.metadata().unwrap().len() as usize,
        ramdisk_load_addr: match vm_arg.ramdisk_load_addr {
            Some(ramdisk_load_addr) => ramdisk_load_addr,
            None => 0,
        },
        ramdisk_img_size: match ramdisk_img {
            Some(ramdisk_file) => ramdisk_file.metadata().unwrap().len() as usize,
            None => 0,
        },
    };

    let fd = open_driver();

    unsafe { ioctl::ioctl(fd, driver_arg) }.expect("failed to perform ioctl");

    // let mut bios_img_buffer = Vec::new();
    // bios_img.read_to_end(&mut bios_img_buffer).unwrap();

    // let mut kernel_img_buffer = Vec::new();
    // kernel_img.read_to_end(&mut kernel_img_buffer).unwrap();

    Ok(())
}

pub fn axvmm_boot_shutdown_vm(is_boot: bool, arg: VmBootShutdownArgs) -> Result<(), String> {
    let vmid = arg.vmid;
    if is_boot {
        println!("Boot VM [{}]", vmid);
    } else {
        println!("Shutdown VM [{}]", vmid);
    }
    Ok(())
}
