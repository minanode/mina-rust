//! Heap accounting for the web node (WN-MEM).
//!
//! A WASM linear memory only grows: `memory.grow` has no inverse, so the size
//! the browser reports is the allocator's high-water mark, not the node's
//! live data. This wraps the standard allocator (dlmalloc on
//! `wasm32-unknown-unknown`) with two counters, live and peak requested bytes,
//! so a soak can tell growing live data from a high-water mark raised by a
//! transient peak or fragmentation. The cost is one relaxed atomic add per
//! allocation and free, and a load on the peak.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use gloo_utils::format::JsValueSerdeExt;
use serde::Serialize;
use wasm_bindgen::prelude::*;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn grew(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed).wrapping_add(n);
    if live > PEAK.load(Relaxed) {
        PEAK.fetch_max(live, Relaxed);
    }
}

fn shrank(n: usize) {
    LIVE.fetch_sub(n, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.dealloc(p, layout) };
        shrank(layout.size());
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let new = unsafe { System.realloc(p, layout, size) };
        if !new.is_null() {
            if size >= layout.size() {
                grew(size - layout.size());
            } else {
                shrank(layout.size() - size);
            }
        }
        new
    }
}

/// What `memory_stats()` returns.
#[derive(Serialize)]
pub struct MemoryStats {
    /// Bytes the node has requested and not freed: its live heap.
    pub live_bytes: usize,
    /// The most `live_bytes` has been since the module started.
    pub peak_live_bytes: usize,
    /// The size of the linear memory: the allocator's high-water mark plus
    /// static data and the main thread's stack. It never shrinks.
    pub wasm_bytes: usize,
}

pub fn stats() -> MemoryStats {
    MemoryStats {
        live_bytes: LIVE.load(Relaxed),
        peak_live_bytes: PEAK.load(Relaxed),
        wasm_bytes: core::arch::wasm32::memory_size(0).saturating_mul(65536),
    }
}

/// The web node's heap counters, as an object. The page can call it at any
/// time, with or without a running node: it reads two counters.
#[wasm_bindgen]
pub fn memory_stats() -> JsValue {
    JsValue::from_serde(&stats()).unwrap_or_default()
}
