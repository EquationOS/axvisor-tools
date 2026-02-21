use libc::{MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE, c_void, mmap};

use equation_defs::page_cache::PAGE_CACHE_POOL_MAGIC;
use equation_defs::{PAGE_CACHE_POOL_BASE_VA, PAGE_CACHE_POOL_SIZE};

/// Setup the page cache region for the instance.
/// This function maps the page cache region into this daemon process's address space,
/// Panic if the global `INSTANCE_FD` is not set.
pub fn setup_shared_page_cache_region() {
    let instance_fd = super::get_instance_fd();

    let page_cache_pool_base = unsafe {
        mmap(
            PAGE_CACHE_POOL_BASE_VA as *mut c_void,
            PAGE_CACHE_POOL_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_SHARED | MAP_FIXED,
            instance_fd,
            (PAGE_CACHE_POOL_MAGIC as i64) << 12,
        )
    };

    assert_ne!(
        page_cache_pool_base,
        MAP_FAILED,
        "Failed to map page cache pool: {}",
        std::io::Error::last_os_error()
    );

    info!(
        "Mapped page cache pool at {:#p}, size: {:#x}",
        page_cache_pool_base, PAGE_CACHE_POOL_SIZE
    );
}
