use libc::{c_void, mmap, MAP_ANONYMOUS, MAP_POPULATE, MAP_PRIVATE, PROT_READ, PROT_WRITE};

use memory_addr::PAGE_SIZE_4K;

pub(super) fn alloc_shared_page(shared_pages: &mut Vec<*mut c_void>) -> *mut c_void {
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
