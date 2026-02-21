use libc::{MAP_ANONYMOUS, MAP_POPULATE, MAP_PRIVATE, PROT_READ, PROT_WRITE, c_void, mmap};

use memory_addr::PAGE_SIZE_4K;

fn alloc_shared_page(shared_pages: &mut Vec<*mut c_void>) -> *mut c_void {
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

pub(super) fn free_shared_pages(shared_pages: &mut Vec<*mut c_void>) {
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

pub(super) fn copy_content_to_shared_pages(shared_pages: &mut Vec<*mut c_void>, src: &[u8]) {
    let mut bytes_left = src.len();
    let mut src_offset = 0;
    let mut current_offset = 0;

    debug!("[*] Copying {} bytes to shared pages", bytes_left);

    while bytes_left > 0 {
        let current_page = alloc_shared_page(shared_pages);
        let page_offset = current_offset % PAGE_SIZE_4K;
        let space_left = PAGE_SIZE_4K - page_offset;
        let to_copy = std::cmp::min(space_left, bytes_left);

        unsafe {
            let dst = (current_page as *mut u8).add(page_offset);
            std::ptr::copy_nonoverlapping(src.as_ptr().add(src_offset), dst, to_copy);
        }

        current_offset += to_copy;
        src_offset += to_copy;
        bytes_left -= to_copy;
    }
    assert!(current_offset == src.len());
}
