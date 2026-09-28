#[cfg(target_os = "macos")]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

// macOS rejects its address-space, data and RSS rlimits. This dedicated
// process therefore bounds all Rust allocations made while parsing an
// untrusted PDF without changing the allocator used by the long-lived daemon.
#[cfg(target_os = "macos")]
const ALLOCATION_LIMIT_BYTES: usize = 512 * 1024 * 1024;

#[cfg(target_os = "macos")]
struct BoundedAllocator;

#[cfg(target_os = "macos")]
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "macos")]
#[global_allocator]
static GLOBAL_ALLOCATOR: BoundedAllocator = BoundedAllocator;

#[cfg(target_os = "macos")]
fn reserve(bytes: usize) -> bool {
    ALLOCATED_BYTES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |allocated| {
            allocated
                .checked_add(bytes)
                .filter(|next| *next <= ALLOCATION_LIMIT_BYTES)
        })
        .is_ok()
}

#[cfg(target_os = "macos")]
fn release(bytes: usize) {
    ALLOCATED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
}

#[cfg(target_os = "macos")]
unsafe impl GlobalAlloc for BoundedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: this wrapper preserves System's allocation contract.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            release(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: this wrapper preserves System's allocation contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if pointer.is_null() {
            release(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout came from System through this wrapper.
        unsafe { System.dealloc(pointer, layout) }
        release(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return std::ptr::null_mut();
        };
        // Allocate the replacement before releasing the old allocation so the
        // budget also covers realloc's transient peak.
        // SAFETY: new_layout is valid and handled by this allocator.
        let replacement = unsafe { self.alloc(new_layout) };
        if replacement.is_null() {
            return replacement;
        }
        // SAFETY: both allocations are valid and non-overlapping; copy only
        // the initialized portion common to their layouts.
        unsafe {
            std::ptr::copy_nonoverlapping(pointer, replacement, layout.size().min(new_size));
            self.dealloc(pointer, layout);
        }
        replacement
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.as_slice() == ["--self-test"] {
        println!("s-code PDF worker self-test ok");
        return Ok(());
    }
    s_code_execution::pdf_text_worker(&arguments)
}
