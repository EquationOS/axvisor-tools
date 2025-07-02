mod daemon;

pub use daemon::poll;

use lazyinit::LazyInit;
use libc::{c_void, mmap, MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE};

use equation_defs::scf::{ScfDescriptor, SyscallQueueBufferMetadata, SCF_QUEUE_BUFF_MAGIC};
use equation_defs::{SCF_QUEUE_BUFF_BASE_VA, SCF_QUEUE_BUFF_SIZE};

static mut SYSCALL_QUEUE_BUFFER: LazyInit<SyscallQueueBuffer> = LazyInit::new();

pub struct SyscallQueueBuffer {
    capacity_mask: u16,
    req_index_last: u16,
    rsp_index_shadow: u16,

    meta: &'static mut SyscallQueueBufferMetadata,
    desc: &'static mut [ScfDescriptor],
    req_ring: &'static mut [u16],
    rsp_ring: &'static mut [u16],
}

impl SyscallQueueBuffer {
    fn get() -> &'static mut Self {
        unsafe { &mut SYSCALL_QUEUE_BUFFER }
    }

    fn has_request(&self) -> bool {
        self.req_index_last != self.meta.req_index()
    }

    fn pop_syscall_request(&mut self) -> Option<(usize, &mut ScfDescriptor)> {
        // spin_lock(&buf->meta->lock);
        if !self.has_request() {
            // spin_unlock(&buf->meta->lock);
            return None;
        }

        let index = self.req_ring[(self.req_index_last & self.capacity_mask) as usize] as usize;
        let desc = &mut self.desc[index];
        self.req_index_last += 1;

        // spin_unlock(&buf->meta->lock);

        Some((index, desc))
    }

    fn push_syscall_response(&mut self, index: usize, ret_val: u64) -> bool {
        if index > self.capacity_mask as usize {
            return false; // Invalid index
        }
        // spin_lock(&buf->meta->lock);

        self.desc[index].set_return_value(ret_val);

        self.rsp_ring[(self.rsp_index_shadow & self.capacity_mask) as usize] = index as u16;
        self.rsp_index_shadow += 1;

        // __sync_synchronize();

        // Update the response index in the metadata
        self.meta.set_rsp_index(self.rsp_index_shadow);

        // spin_unlock(&buf->meta->lock);

        true
    }
}

pub fn setup_syscall_proxy_queue_buffer(instance_fd: i32) {
    let syscall_queue_base = unsafe {
        mmap(
            SCF_QUEUE_BUFF_BASE_VA as *mut c_void,
            SCF_QUEUE_BUFF_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_SHARED | MAP_FIXED,
            instance_fd,
            // Offset in bytes
            // Use as magic number to tell the kernel driver
            // this is a syscall queue buffer
            (SCF_QUEUE_BUFF_MAGIC as i64) << 12,
        )
    };
    assert_ne!(
        syscall_queue_base,
        MAP_FAILED,
        "Failed to map syscall queue buffer: {}",
        std::io::Error::last_os_error()
    );

    let meta = SyscallQueueBufferMetadata::construct_mut();

	// Check if the magic number is valid.
	// This may trigger a page_fault if the memory is not mapped correctly.
    assert!(
        meta.is_valid(),
        "Invalid magic number for syscall queue buffer"
    );

    let desc = meta.descriptor_table();
    let req_ring = meta.request_ring();
    let rsp_ring = meta.response_ring();

    unsafe {
        SYSCALL_QUEUE_BUFFER.init_once(SyscallQueueBuffer {
            capacity_mask: meta.capacity() - 1,
            req_index_last: 0,
            rsp_index_shadow: meta.rsp_index(),
            meta,
            desc,
            req_ring,
            rsp_ring,
        });
    }
}
