use std::ffi::CStr;

use libc::c_void;

use crate::hvc::{hvc_create_instance, hvc_init_shim, hvc_setup_instance};
use crate::loader;
use crate::shared_pages::{copy_content_to_shared_pages, free_shared_pages};
use crate::{ExecuteArgs, InstanceCreateArgs};

const PIE_BASE: usize = 0x40000000;
const LDSO_BASE: usize = 0x7f0000000000;

const EQINSTANCE_DEV_PREFIX: &str = "/dev/eqinstance_";

pub fn create_instance(args: InstanceCreateArgs) {
    info!(
        "Create {} instance with file path: {:?}",
        match args.instance_type {
            0 => "LibOS",
            1 => "Kernel",
            _ => panic!("Invalid instance type"),
        },
        args.file_path
    );

    let raw_file = std::fs::read(args.file_path).expect("Failed to read ELF file");

    // Copy the raw file to axvisor through shared pages
    // page by page.
    let mut shared_pages: Vec<*mut c_void> = Vec::new();
    copy_content_to_shared_pages(&mut shared_pages, raw_file.as_slice());

    let iid = hvc_create_instance(
        args.instance_type as _,
        if args.one2onemapping { 0 } else { 1 } as _,
        raw_file.len() as _,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as _,
    );

    if iid < 0 {
        panic!("Failed to create instance: {}", iid);
    }

    info!("Create instance success, instance ID = [{}]", iid);

    free_shared_pages(&mut shared_pages);
}

pub fn load_junction(args: ExecuteArgs) {
    use linux_libc_auxv::{AuxVar, AuxVarFlags};
    use pi_memory_layout::{ArgsLayoutBuilder, ArgsLayoutRef};

    let instance_id = crate::ioctl::ioctl_create_instance()
        .expect("Failed to create instance for dynamic loading");

    info!("Create instance success, instance ID = [{}]", instance_id);

    let instance_dev_path_str = format!("{}{}\0", EQINSTANCE_DEV_PREFIX, instance_id);
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
    }

    let fd = Some(instance_fd);
    let elf_path = args.exec_args[0].clone();

    let (app_elf, app_base, _, interp_path) =
        unsafe { loader::elf::mmap_elf(elf_path, PIE_BASE, fd) };

    let (ldso_elf, ldso_base) = if let Some(interp_path) = interp_path {
        let (ldso_elf, ldso_base, _, path) =
            unsafe { loader::elf::mmap_elf(&interp_path, LDSO_BASE, fd) };
        if let Some(path) = path {
            panic!(
                "[*] Found interpreter: {:?} for interp {:?}",
                path, interp_path
            );
        }
        (ldso_elf, ldso_base)
    } else {
        panic!("[*] No interpreter found in junction ELF");
    };

    let is_app_pie = app_elf.header.e_type == goblin::elf::header::ET_DYN;

    // We need to copy execution metadate to axvisor to start the loader instance.
    let mut args_builder = ArgsLayoutBuilder::new();
    for arg in &args.exec_args {
        args_builder.add_argv(arg);
    }
    args_builder.add_envv("EQTEST=1"); // Set EQTEST environment variable

    let (entry, phdr) = if is_app_pie {
        (
            app_base + app_elf.entry as usize,
            app_base + app_elf.header.e_phoff as usize,
        )
    } else {
        (app_elf.entry as usize, app_elf.header.e_phoff as usize)
    };

    let mut random_bytes = [0u8; 16];
    for (index, byte) in random_bytes.iter_mut().enumerate() {
        *byte = index as u8; // Fill with dummy data for now
    }

    args_builder.add_auxv(AuxVar::Pagesz(4096)); // 4KB page size
    args_builder.add_auxv(AuxVar::Phdr(phdr as *mut u8));
    args_builder.add_auxv(AuxVar::Phent(app_elf.header.e_phentsize as usize));
    args_builder.add_auxv(AuxVar::Phnum(app_elf.header.e_phnum as usize));
    args_builder.add_auxv(AuxVar::Entry(entry as *mut u8));
    args_builder.add_auxv(AuxVar::Flags(AuxVarFlags::NOT_PRESERVE_ARGV0));
    // args_builder.add_auxv(AuxVar::Random(random_bytes));
    args_builder.add_auxv(AuxVar::Base(ldso_base as *mut u8)); // Base address for ld.so

    let args_layout = args_builder.build();

    if true {
        // Print the stack layout for debugging purposes
        let layout = ArgsLayoutRef::new(args_layout.as_ref(), None);

        for (i, arg) in unsafe { layout.argv_iter() }.enumerate() {
            println!("  [{i}] {}", arg.to_str().unwrap());
        }
        for (i, env) in unsafe { layout.envv_iter() }.enumerate() {
            println!("  [env {i}] {}", env.to_str().unwrap());
        }
        for auxv in unsafe { layout.auxv_iter() } {
            println!("  [auxv] {:?}", auxv);
        }
    }

    let ld_entry = ldso_base + ldso_elf.entry as usize;

    // Copy the stack layout to axvisor through shared pages
    // page by page.
    let mut shared_pages: Vec<*mut c_void> = Vec::new();
    copy_content_to_shared_pages(&mut shared_pages, args_layout.as_ref());

    let res = hvc_setup_instance(
        instance_id as _,
        args_layout.len() as u64,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as u64,
        ld_entry as u64,
    );
    if res < 0 {
        panic!("Failed to setup instance: {}", res);
    }

    info!("Setup instance success, instance ID = [{}]", instance_id);

    free_shared_pages(&mut shared_pages);

    // At this point, we just passed the stack layout (the arguments and environment variables)
    // to the axvisor, and the axvisor will handle the loading of the junction.
    // In the next step, this process will turn into a proxy process of the junction instance,
    // which handles the system calls which can not be handled by the axvisor directly.
}

pub fn remove_instance(instance_id: u64) {
    info!("Remove instance with ID: {}", instance_id);

    crate::ioctl::ioctl_remove_instance(instance_id).expect("Failed to remove instance");

    info!("Instance with ID {} removed successfully", instance_id);
}

pub fn init_shim() {
    info!("Init shim");
    hvc_init_shim();
}
