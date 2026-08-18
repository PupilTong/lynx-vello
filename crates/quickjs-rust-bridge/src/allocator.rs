//! C allocation hooks used by the pinned `QuickJS` build.

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc};
use std::ffi::c_void;
use std::ptr;

const C_ALIGNMENT: usize = 16;

#[repr(C, align(16))]
struct AllocationHeader {
    requested_size: usize,
}

const HEADER_SIZE: usize = size_of::<AllocationHeader>();
const _: () = assert!(HEADER_SIZE == C_ALIGNMENT);

fn allocation_layout(requested_size: usize) -> Option<Layout> {
    let payload_size = requested_size.max(1);
    let allocation_size = HEADER_SIZE.checked_add(payload_size)?;
    Layout::from_size_align(allocation_size, C_ALIGNMENT).ok()
}

unsafe fn allocation_base(pointer: *mut c_void) -> *mut u8 {
    // SAFETY: every non-null pointer accepted by this private ABI was returned
    // by `allocate` below and therefore has an `AllocationHeader` immediately
    // before it.
    unsafe { pointer.cast::<u8>().sub(HEADER_SIZE) }
}

unsafe fn allocation_header(pointer: *mut c_void) -> *mut AllocationHeader {
    // SAFETY: upheld by the caller and forwarded to `allocation_base`.
    unsafe { allocation_base(pointer).cast() }
}

#[allow(
    clippy::cast_ptr_alignment,
    reason = "alloc returns the 16-byte alignment requested by the Layout"
)]
unsafe fn allocate(requested_size: usize, zeroed: bool) -> *mut c_void {
    let Some(layout) = allocation_layout(requested_size) else {
        return ptr::null_mut();
    };
    // SAFETY: `layout` is non-zero and valid. A null result is passed through
    // with normal C allocator semantics.
    let base = unsafe {
        if zeroed {
            alloc_zeroed(layout)
        } else {
            alloc(layout)
        }
    };
    if base.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: `base` denotes `layout.size()` writable bytes aligned for the
    // header, whose size is exactly the payload offset.
    unsafe {
        base.cast::<AllocationHeader>()
            .write(AllocationHeader { requested_size });
        base.add(HEADER_SIZE).cast()
    }
}

/// Allocates C storage through Rust's process global allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_malloc(size: usize) -> *mut c_void {
    // SAFETY: the helper returns an owned C allocation or null.
    unsafe { allocate(size, false) }
}

/// Allocates zeroed C storage through Rust's process global allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        return ptr::null_mut();
    };
    // SAFETY: the helper returns an owned C allocation or null.
    unsafe { allocate(total, true) }
}

/// Resizes C storage through Rust's process global allocator.
#[unsafe(no_mangle)]
#[allow(
    clippy::cast_ptr_alignment,
    reason = "realloc preserves the 16-byte alignment requested by the Layout"
)]
pub unsafe extern "C" fn qjs_rust_realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    if pointer.is_null() {
        // SAFETY: null `realloc` has `malloc` semantics.
        return unsafe { allocate(size, false) };
    }
    if size == 0 {
        // SAFETY: `pointer` belongs to this allocator by this private ABI's
        // contract. C permits `realloc(pointer, 0)` to free and return null.
        unsafe { qjs_rust_free(pointer) };
        return ptr::null_mut();
    }

    // SAFETY: the private C boundary never passes a pointer from another
    // allocator. Its prefix therefore contains the original requested size.
    let header = unsafe { allocation_header(pointer) };
    // SAFETY: `header` points into the live allocation described above.
    let old_size = unsafe { (*header).requested_size };
    let Some(old_layout) = allocation_layout(old_size) else {
        return ptr::null_mut();
    };
    let Some(new_layout) = allocation_layout(size) else {
        return ptr::null_mut();
    };

    // SAFETY: `header` is the base pointer returned for `old_layout`. On
    // failure Rust leaves the old allocation live, matching C `realloc`.
    let new_base = unsafe { realloc(header.cast(), old_layout, new_layout.size()) };
    if new_base.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: the resized block is still aligned to `C_ALIGNMENT` and has
    // room for the header followed by `size` payload bytes.
    unsafe {
        new_base.cast::<AllocationHeader>().write(AllocationHeader {
            requested_size: size,
        });
        new_base.add(HEADER_SIZE).cast()
    }
}

/// Frees C storage through Rust's process global allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }

    // SAFETY: the private C boundary only releases pointers allocated by the
    // functions in this module.
    let header = unsafe { allocation_header(pointer) };
    // SAFETY: the prefix remains initialized for the lifetime of the block.
    let requested_size = unsafe { (*header).requested_size };
    let Some(layout) = allocation_layout(requested_size) else {
        return;
    };
    // SAFETY: `header` is the original base pointer and `layout` is identical
    // to the layout used for the most recent allocation/reallocation.
    unsafe { dealloc(header.cast(), layout) };
}

/// Returns the payload size tracked for a C allocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qjs_rust_malloc_usable_size(pointer: *const c_void) -> usize {
    if pointer.is_null() {
        return 0;
    }
    // SAFETY: the private C boundary only queries pointers allocated here.
    unsafe { (*allocation_header(pointer.cast_mut())).requested_size }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malloc_is_aligned_and_reports_its_payload_size() {
        for size in [0, 1, 15, 16, 17, 4096] {
            // SAFETY: this test pairs allocations with this module's own free.
            let pointer = unsafe { qjs_rust_malloc(size) };
            assert!(!pointer.is_null());
            assert_eq!((pointer as usize) % C_ALIGNMENT, 0);
            // SAFETY: `pointer` is a live allocation from this module.
            assert_eq!(unsafe { qjs_rust_malloc_usable_size(pointer) }, size);
            // SAFETY: `pointer` has not been released yet.
            unsafe { qjs_rust_free(pointer) };
        }
    }

    #[test]
    fn calloc_zeroes_and_rejects_overflow() {
        // SAFETY: the arguments are ordinary C allocation inputs.
        let pointer = unsafe { qjs_rust_calloc(32, 4) };
        assert!(!pointer.is_null());
        // SAFETY: the allocation contains 128 initialized payload bytes.
        let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), 128) };
        assert!(bytes.iter().all(|byte| *byte == 0));
        // SAFETY: `pointer` has not been released yet.
        unsafe { qjs_rust_free(pointer) };

        // SAFETY: overflow is handled before attempting allocation.
        assert!(unsafe { qjs_rust_calloc(usize::MAX, 2) }.is_null());
    }

    #[test]
    fn realloc_preserves_data_and_failure_keeps_the_old_block_live() {
        // SAFETY: this test retains and releases the allocation exclusively.
        let pointer = unsafe { qjs_rust_malloc(8) };
        assert!(!pointer.is_null());
        // SAFETY: `pointer` exposes eight writable payload bytes.
        unsafe { ptr::write_bytes(pointer, 0x5a, 8) };

        // SAFETY: `pointer` is a live allocation from this module.
        let grown = unsafe { qjs_rust_realloc(pointer, 64) };
        assert!(!grown.is_null());
        // SAFETY: a successful realloc preserves the first eight bytes.
        let bytes = unsafe { std::slice::from_raw_parts(grown.cast::<u8>(), 8) };
        assert!(bytes.iter().all(|byte| *byte == 0x5a));
        // SAFETY: `grown` is live and its impossible resize must leave it live.
        assert!(unsafe { qjs_rust_realloc(grown, usize::MAX) }.is_null());
        assert_eq!(unsafe { qjs_rust_malloc_usable_size(grown) }, 64);
        // SAFETY: the failed realloc did not release `grown`.
        unsafe { qjs_rust_free(grown) };
    }
}
