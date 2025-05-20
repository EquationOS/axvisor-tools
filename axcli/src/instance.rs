use libc::c_void;
use memory_addr::PAGE_SIZE_4K;

use crate::hvc::{hvc_create_instance, hvc_init_shim};
use crate::shared_pages::{alloc_shared_page, free_shared_pages};
use crate::InstanceCreateArgs;

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

    let mut bytes_left = raw_file.len();
    let mut src_offset = 0;
    let mut current_offset = 0;
    let mut shared_pages: Vec<*mut c_void> = Vec::new();

    while bytes_left > 0 {
        let current_page = alloc_shared_page(&mut shared_pages);
        let page_offset = current_offset % PAGE_SIZE_4K;
        let space_left = PAGE_SIZE_4K - page_offset;
        let to_copy = std::cmp::min(space_left, bytes_left);

        unsafe {
            let dst = (current_page as *mut u8).add(page_offset);
            std::ptr::copy_nonoverlapping(raw_file.as_ptr().add(src_offset), dst, to_copy);
        }

        current_offset += to_copy;
        src_offset += to_copy;
        bytes_left -= to_copy;
    }
    assert!(current_offset == raw_file.len());

    let iid = hvc_create_instance(
        args.instance_type as _,
        if args.one2onemapping { 0 } else { 1 } as _,
        current_offset as _,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as _,
    );

    if iid < 0 {
        panic!("Failed to create instance: {}", iid);
    }

    info!("Create instance success, instance ID = [{}]", iid);

    free_shared_pages(&mut shared_pages);
}

pub fn init_shim() {
    info!("Init shim");
    hvc_init_shim();
}
