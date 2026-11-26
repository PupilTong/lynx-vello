//! Returned bytes are released by the `ArrayBuffer` that took them.
//!
//! A host function's returned `HostValue::Bytes` becomes the storage of an
//! external `ArrayBuffer`, which a runtime's memory limit does not count, so
//! a leak of it would not surface as a `QuickJS` out-of-memory failure the way
//! the leak tests in `host_functions.rs` rely on. This binary counts the
//! bytes live in the process allocator instead, so it holds one test only.

#![allow(
    unsafe_code,
    reason = "a counting global allocator forwards to the system allocator"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use quickjs_rust_bridge::{EvalOptions, EvalSource, HostValue, Runtime};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

#[allow(clippy::cast_possible_wrap, reason = "an allocation fits in isize")]
// SAFETY: every call forwards to `System` with the caller's own arguments.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const CHUNK: usize = 1024 * 1024;

#[test]
fn returned_buffers_are_freed_by_detach_transfer_and_collection() {
    let mut runtime = Runtime::new().unwrap();
    let mut realm = runtime.create_context().unwrap();
    realm
        .define_global_function("make", 0, |_| {
            Ok(HostValue::Bytes(vec![1u8; CHUNK].into_boxed_slice()))
        })
        .unwrap();
    let round = "for (let i = 0; i < 64; i++) { \
                   const a = make(); \
                   if (i % 4 === 1) a.transfer(); \
                   if (i % 4 === 2) a.transfer(16); \
                   if (i % 4 === 3) globalThis.last = a.transfer(); \
                 }";
    realm
        .evaluate(EvalSource::new(round), EvalOptions::default())
        .unwrap();
    runtime.run_gc();
    let before = LIVE.load(Ordering::Relaxed);
    for _ in 0..8 {
        realm
            .evaluate(EvalSource::new(round), EvalOptions::default())
            .unwrap();
        runtime.run_gc();
    }
    let grown = LIVE.load(Ordering::Relaxed) - before;
    #[allow(clippy::cast_possible_wrap, reason = "a small constant")]
    let bound = 4 * CHUNK as isize;
    assert!(
        grown < bound,
        "{grown} bytes stayed live after 8 rounds of returned buffers"
    );
}
