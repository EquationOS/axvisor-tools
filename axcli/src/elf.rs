use std::fs::File;
use std::os::fd::AsRawFd;

use libc::{
    c_int, c_void, memset, mmap, MAP_ANONYMOUS, MAP_FIXED_NOREPLACE, MAP_POPULATE, MAP_PRIVATE,
    MAP_SHARED, PROT_EXEC, PROT_READ, PROT_WRITE,
};
use xmas_elf::program::Type;
use xmas_elf::{header, ElfFile};

use memory_addr::{align_down, align_up, PAGE_SIZE_4K};
use page_table_multiarch::MappingFlags;

use crate::hvc::hvc_create_instance;

fn init_shared_page(shared_pages: &mut Vec<*mut c_void>) -> *mut c_void {
    let page = unsafe {
        mmap(
            0 as *mut c_void,
            PAGE_SIZE_4K,
            PROT_READ | PROT_WRITE,
            MAP_ANONYMOUS | MAP_PRIVATE | MAP_POPULATE,
            -1,
            0,
        )
    };

    if page == libc::MAP_FAILED {
        panic!(
            "Failed to allocate shared page: {}",
            std::io::Error::last_os_error()
        );
    }

    shared_pages.push(page);
    page
}

fn dump_shared_pages(shared_pages: &Vec<*mut c_void>, total_count: usize) {
    let mut i = 0;
    for (page_idx, page) in shared_pages.iter().enumerate() {
        let length = if page_idx == shared_pages.len() - 1 {
            total_count % MAX_REGIONS_PER_PAGE
        } else {
            MAX_REGIONS_PER_PAGE
        };

        let page_data =
            unsafe { std::slice::from_raw_parts(*page as *const ELFMemoryRegion, length) };
        for region in page_data {
            info!("[{}] {:#x?}", i, region);
            i += 1;
        }
    }

    if i != total_count {
        panic!(
            "Mismatch in total count of ELF memory regions: expected {}, found {}",
            total_count, i
        );
    }
}

fn free_shared_pages(shared_pages: &mut Vec<*mut c_void>) {
    for page in shared_pages.iter() {
        unsafe {
            if libc::munmap(*page, PAGE_SIZE_4K) != 0 {
                panic!(
                    "Failed to unmap shared page: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }
    shared_pages.clear();
}

pub fn copy_file(path: &str) {
    let mut total_count = 0;
    let mut current_offset = 0;
    let mut shared_pages: Vec<*mut c_void> = Vec::new();

    let mut current_page = init_shared_page(&mut shared_pages);

    let raw_file_path = File::open(path).expect("Failed to open ELF file");

    let raw_file = std::fs::read(path).expect("Failed to read ELF file");

    let elf = ElfFile::new(&raw_file).expect("Failed to parse ELF file");

    assert_eq!(
        elf.header.pt2.type_().as_type(),
        header::Type::Executable,
        "ELF is not an executable object"
    );

    for ph in elf.program_iter() {
        if ph.get_type() != Ok(Type::Load) {
            continue;
        }

        info!(
            "Mapping segment: type={:?}, flags={:?}, offset={:#x}, vaddr={:#x}, paddr={:#x}, file_size={:#x}, mem_size={:#x}",
            ph.get_type(),
            ph.flags(),
            ph.offset(),
            ph.virtual_addr(),
            ph.physical_addr(),
            ph.file_size(),
            ph.mem_size()
        );

        let mut prot: c_int = 0;
        let mut mapping_flags: MappingFlags = MappingFlags::USER;
        if ph.flags().is_execute() {
            prot |= PROT_EXEC;
            mapping_flags |= MappingFlags::EXECUTE;
        }
        if ph.flags().is_write() {
            prot |= PROT_WRITE;
            mapping_flags |= MappingFlags::WRITE;
        }
        if ph.flags().is_read() {
            prot |= PROT_READ;
            mapping_flags |= MappingFlags::READ;
        }

        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let file_size = align_up(ph.file_size() as usize, page_size);

        let start = align_down(ph.virtual_addr() as usize, page_size);

        // Map the file part of the segment.
        let vaddr = unsafe {
            mmap(
                start as *mut c_void,
                file_size,
                prot,
                MAP_FIXED_NOREPLACE | MAP_POPULATE | MAP_SHARED,
                raw_file_path.as_raw_fd(),
                align_down(ph.offset() as usize, page_size) as libc::off_t,
            )
        };

        if vaddr == libc::MAP_FAILED {
            panic!(
                "Failed to map memory for ELF segment: {}",
                std::io::Error::last_os_error()
            );
        }

        info!("Mapped ELF segment at address: {:#x?}", vaddr);

        let elf_region = ELFMemoryRegion {
            start: start as u64,
            end: start as u64 + file_size as u64,
            flags: mapping_flags.bits() as u64,
        };

        if current_offset >= MAX_REGIONS_PER_PAGE {
            current_offset = 0;
            current_page = init_shared_page(&mut shared_pages) as *mut ELFMemoryRegion;
        }
        unsafe {
            // Write the ELF memory region to the shared page.
            core::ptr::write(current_page.add(current_offset), elf_region);
        }
        current_offset += 1;
        total_count += 1;
    }

    dump_shared_pages(&shared_pages, total_count);

    hvc_create_instance(
        0,
        total_count as _,
        shared_pages.as_ptr() as u64,
        shared_pages.len() as _,
        elf.header.pt2.entry_point(),
        if one2onemapping { 0 } else { 1 },
    );

    free_shared_pages(&mut shared_pages);
}
