//! Requested live heap bytes for the single-threaded block replay harness.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Mutex,
    },
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static PHASE_PEAK: AtomicUsize = AtomicUsize::new(0);
static PHASES: Mutex<PhaseMeasurements> = Mutex::new(PhaseMeasurements {
    start: (0, 0),
    rows: [(0, 0, 0); 7],
});

struct PhaseMeasurements {
    start: (usize, usize),
    rows: [(isize, usize, usize); 7],
}

pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            added(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                added(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        new_ptr
    }
}

fn added(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
    PHASE_PEAK.fetch_max(live, Relaxed);
    TOTAL.fetch_add(size, Relaxed);
    LARGEST.fetch_max(size, Relaxed);
}

/// The ledger calls this after each apply stage. Stage zero starts the first
/// interval; stages one through seven finish one interval and start the next.
pub fn checkpoint(stage: u8) {
    let mut phases = PHASES.lock().unwrap();
    let live = LIVE.load(Relaxed);
    let total = TOTAL.load(Relaxed);
    if stage != 0 {
        let (start_live, start_total) = phases.start;
        phases.rows[usize::from(stage - 1)] = (
            live as isize - start_live as isize,
            PHASE_PEAK.load(Relaxed).saturating_sub(start_live),
            total.saturating_sub(start_total),
        );
    }
    phases.start = (live, total);
    PHASE_PEAK.store(live, Relaxed);
}

pub fn phase_rows() -> [(isize, usize, usize); 7] {
    PHASES.lock().unwrap().rows
}

pub fn start() -> (usize, usize) {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    LARGEST.store(0, Relaxed);
    (live, TOTAL.load(Relaxed))
}

pub fn sample((start_live, start_total): (usize, usize)) -> (isize, usize, usize, usize) {
    let live = LIVE.load(Relaxed);
    let peak = PEAK.load(Relaxed);
    (
        live as isize - start_live as isize,
        peak.saturating_sub(start_live),
        TOTAL.load(Relaxed).saturating_sub(start_total),
        LARGEST.load(Relaxed),
    )
}
