//! Bounded-time, structure-aware fuzzing of the code that takes peer input.
//!
//! A peer controls every byte it sends us. Whatever it sends, the node must
//! reject what is malformed as invalid input: never panic, never allocate
//! for a length the bytes don't back, and never stall. [`fuzz`] checks that
//! for one entry point: it starts from real captured messages, mutates them
//! the ways that reach the decoders' edge cases, and runs the entry point on
//! each mutant until the time budget is spent.
//!
//! The mutations follow bin_prot's encoding: messages cut short, integers and
//! length prefixes replaced with large ones, 32-byte field elements and
//! points set at or above the field's modulus or to zero, variant tags
//! changed, and spans deleted, repeated or spliced in from other messages.
//!
//! The budget comes from the environment, so CI can keep it short and a
//! local run can go on for hours:
//!
//! - `PEER_INPUT_FUZZ_SECS`: seconds per entry point (default 5);
//! - `PEER_INPUT_FUZZ_SEED`: the seed (default fixed, for repeatable runs);
//! - `PEER_INPUT_FUZZ_OUT`: a directory to save a failing input to;
//! - `PEER_INPUT_FUZZ_REPLAY`: a saved input to run alone, to reproduce.
//!
//! A test binary that uses it installs [`TrackingAllocator`] as its global
//! allocator, which lets [`fuzz`] check what each case allocates.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    panic::{catch_unwind, AssertUnwindSafe},
    time::{Duration, Instant},
};

use rand::{rngs::SmallRng, Rng, SeedableRng};

/// Pallas base field modulus, little-endian: `Fp` in the node.
const FP_MODULUS: [u8; 32] = modulus(0x992d30ed00000001, 0x224698fc094cf91b);
/// Pallas scalar field modulus, little-endian: `Fq` in the node.
const FQ_MODULUS: [u8; 32] = modulus(0x8c46eb2100000001, 0x224698fc0994a8dd);

const fn modulus(low: u64, mid: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let (low, mid, high) = (
        low.to_le_bytes(),
        mid.to_le_bytes(),
        (1u64 << 62).to_le_bytes(),
    );
    let mut i = 0;
    while i < 8 {
        bytes[i] = low[i];
        bytes[8 + i] = mid[i];
        bytes[24 + i] = high[i];
        i += 1;
    }
    bytes
}

/// One-byte values at the edges of bin_prot's encodings: integer prefixes,
/// booleans and option tags, and the largest values.
const INTERESTING_BYTES: [u8; 10] = [0, 1, 2, 0x7f, 0x80, 0xfb, 0xfc, 0xfd, 0xfe, 0xff];

/// Mutates encoded messages the ways that reach decoders' edge cases.
pub struct Mutator {
    rng: SmallRng,
}

impl Mutator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    pub fn rng(&mut self) -> &mut SmallRng {
        &mut self.rng
    }

    /// One to three mutations of `input`; `others` are for splicing.
    pub fn mutate(&mut self, input: &[u8], others: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = input.to_vec();
        for _ in 0..self.rng.gen_range(1..=3) {
            self.mutate_once(&mut bytes, others);
        }
        bytes
    }

    fn offset(&mut self, len: usize) -> usize {
        // Message headers and the last bytes are where framing lives.
        match self.rng.gen_range(0..8) {
            0 => self.rng.gen_range(0..len.min(64)),
            1 => len - 1 - self.rng.gen_range(0..len.min(64)),
            _ => self.rng.gen_range(0..len),
        }
    }

    fn span(&mut self, len: usize, at: usize) -> usize {
        let max = (len - at).min(match self.rng.gen_range(0..4) {
            0 => 4096,
            _ => 64,
        });
        self.rng.gen_range(1..=max)
    }

    /// A bin_prot integer far larger than any length the input backs.
    pub fn large_integer(&mut self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(9);
        match self.rng.gen_range(0..5) {
            0 => {
                bytes.push(0xfc);
                bytes.extend_from_slice(&((1u64 << 62) - 1).to_le_bytes());
            }
            1 => {
                bytes.push(0xfc);
                let v: u64 = self.rng.gen_range(1 << 32..1 << 62);
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            2 => {
                bytes.push(0xfd);
                bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            }
            3 => {
                bytes.push(0xfd);
                let v: u32 = self.rng.gen_range(1 << 20..u32::MAX);
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            _ => {
                bytes.push(0xfe);
                bytes.extend_from_slice(&u16::MAX.to_le_bytes());
            }
        }
        bytes
    }

    /// 32 bytes that are not a field element: at or above a modulus.
    pub fn out_of_range_field(&mut self) -> [u8; 32] {
        let mut bytes = match self.rng.gen_range(0..5) {
            0 => FP_MODULUS,
            1 => FQ_MODULUS,
            2 => [0xff; 32],
            _ => {
                let mut bytes = [0u8; 32];
                self.rng.fill(&mut bytes[..]);
                bytes[31] |= 0x80;
                bytes
            }
        };
        if self.rng.gen_bool(0.25) {
            bytes[0] = bytes[0].wrapping_add(1);
        }
        bytes
    }

    fn overwrite(bytes: &mut [u8], at: usize, with: &[u8]) {
        let end = (at + with.len()).min(bytes.len());
        bytes[at..end].copy_from_slice(&with[..end - at]);
    }

    fn mutate_once(&mut self, bytes: &mut Vec<u8>, others: &[Vec<u8>]) {
        if bytes.is_empty() {
            bytes.push(self.rng.gen());
            return;
        }
        let at = self.offset(bytes.len());
        match self.rng.gen_range(0..12) {
            0 => bytes.truncate(at),
            1 => bytes[at] ^= 1 << self.rng.gen_range(0..8),
            2 => bytes[at] = INTERESTING_BYTES[self.rng.gen_range(0..INTERESTING_BYTES.len())],
            3 => {
                let integer = self.large_integer();
                Self::overwrite(bytes, at, &integer);
            }
            4 => {
                let integer = self.large_integer();
                bytes.splice(at..at, integer);
            }
            5 => {
                let field = self.out_of_range_field();
                Self::overwrite(bytes, at, &field);
            }
            // A point or field of zeros: the point at infinity, a zero key.
            6 => Self::overwrite(bytes, at, &[0; 64]),
            7 => {
                let len = self.span(bytes.len(), at);
                bytes.drain(at..at + len);
            }
            8 => {
                let len = self.span(bytes.len(), at);
                let copy = bytes[at..at + len].to_vec();
                bytes.splice(at..at, copy);
            }
            9 if !others.is_empty() => {
                let other = &others[self.rng.gen_range(0..others.len())];
                if !other.is_empty() {
                    let from = self.rng.gen_range(0..other.len());
                    let len = self.span(other.len(), from);
                    bytes.splice(at..at, other[from..from + len].iter().copied());
                }
            }
            10 => {
                let mut random = vec![0u8; self.span(bytes.len(), at)];
                self.rng.fill(&mut random[..]);
                Self::overwrite(bytes, at, &random);
            }
            _ => {
                // A variant tag or small integer one past what is valid.
                bytes[at] = bytes[at].wrapping_add(self.rng.gen_range(1..4));
            }
        }
    }
}

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

/// No decoder should ever ask for this much at once: it means a length was
/// trusted. Failing the allocation aborts at once, with its size, rather than
/// letting the machine's overcommit decide.
pub const REFUSED_ALLOCATION: usize = 1 << 34;

/// The system allocator, counting what each thread allocates so [`fuzz`] can
/// check each case. Install it with `#[global_allocator]`.
pub struct TrackingAllocator;

impl TrackingAllocator {
    fn record(size: usize, grow: isize) -> bool {
        // Const-initialised thread locals without destructors never allocate.
        let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        let _ = LIVE.try_with(|live| {
            live.set(live.get() + grow);
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
        });
        size <= REFUSED_ALLOCATION
    }

    fn forget(size: usize) {
        let _ = LIVE.try_with(|live| live.set(live.get() - size as isize));
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::record(layout.size(), layout.size() as isize) {
            return std::ptr::null_mut();
        }
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !Self::record(layout.size(), layout.size() as isize) {
            return std::ptr::null_mut();
        }
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::forget(layout.size());
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if !Self::record(new_size, new_size as isize - layout.size() as isize) {
            return std::ptr::null_mut();
        }
        System.realloc(ptr, layout, new_size)
    }
}

/// What one case allocated on its thread.
#[derive(Debug, Clone, Copy)]
pub struct Allocation {
    /// The largest single request.
    pub largest: usize,
    /// The most held at once, above what was held when the case started.
    pub peak: usize,
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, Allocation) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(start));
    LARGEST.with(|largest| largest.set(0));
    let value = f();
    let allocation = Allocation {
        largest: LARGEST.with(Cell::get),
        peak: (PEAK.with(Cell::get) - start).max(0) as usize,
    };
    (value, allocation)
}

/// Whether a saved input is being replayed instead of fuzzing: then the
/// callers' checks that the corpus was accepted don't apply.
pub fn replaying() -> bool {
    std::env::var_os("PEER_INPUT_FUZZ_REPLAY").is_some()
}

/// The input saved for `name`, when `PEER_INPUT_FUZZ_REPLAY` names a file
/// [`fuzz`] saved for it: only that input is run.
fn replay(name: &str) -> Option<Vec<u8>> {
    let path = std::path::PathBuf::from(std::env::var_os("PEER_INPUT_FUZZ_REPLAY")?);
    let file = path.file_name()?.to_str()?;
    let saved_for = file.rsplitn(3, '-').nth(2)?;
    (saved_for == name).then(|| std::fs::read(&path).expect("PEER_INPUT_FUZZ_REPLAY"))
}

/// How long to fuzz, and with what seed.
#[derive(Debug, Clone)]
pub struct Budget {
    pub per_entry_point: Duration,
    /// A single case that takes longer than this has stalled.
    pub per_case: Duration,
    pub seed: u64,
}

impl Budget {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
        Self {
            per_entry_point: Duration::from_secs(var("PEER_INPUT_FUZZ_SECS").unwrap_or(5)),
            per_case: Duration::from_secs(var("PEER_INPUT_FUZZ_CASE_SECS").unwrap_or(20)),
            seed: var("PEER_INPUT_FUZZ_SEED").unwrap_or(0x5eed_f00d),
        }
    }
}

/// What a run of [`fuzz`] did.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub cases: u64,
    /// Cases the entry point accepted.
    pub accepted: u64,
    pub largest_allocation: usize,
    pub peak_allocation: usize,
    pub slowest_case: Duration,
}

/// The limits a case must stay within: allocation in proportion to its
/// input, with room for the fixed costs of decoding and checking.
fn allocation_limit(input: usize) -> (usize, usize) {
    let largest = (64 << 20) + 16 * input;
    let peak = (512 << 20) + 64 * input;
    (largest, peak)
}

fn fnv(name: &str) -> u64 {
    name.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

/// Fuzzes `entry_point` with mutants of `corpus` for the budget's time.
///
/// `entry_point` runs the code under test on one input and says whether the
/// input was accepted; any rejection must be an `Err` or `false`, never a
/// panic. Every corpus entry is run first, unmutated. A case fails if it
/// panics, takes longer than the budget allows a case, or allocates more
/// than its input warrants; the failure names the seed and case, and the
/// input is saved to `PEER_INPUT_FUZZ_OUT` if that is set.
pub fn fuzz(
    name: &str,
    corpus: &[Vec<u8>],
    budget: &Budget,
    mut entry_point: impl FnMut(&[u8]) -> bool,
) -> Report {
    assert!(!corpus.is_empty(), "{name}: empty corpus");
    if let Some(input) = replay(name) {
        // Outside `catch_unwind`, so a panic shows its backtrace.
        let accepted = entry_point(&input);
        eprintln!(
            "{name}: replayed {} bytes, accepted: {accepted}",
            input.len()
        );
        return Report {
            cases: 1,
            accepted: accepted as u64,
            ..Default::default()
        };
    }
    let seed = budget.seed ^ fnv(name);
    let mut mutator = Mutator::new(seed);
    let mut report = Report::default();
    let start = Instant::now();
    let mut index = 0u64;
    loop {
        let input = match corpus.get(index as usize) {
            Some(entry) => entry.clone(),
            None if start.elapsed() >= budget.per_entry_point => break,
            None => {
                let entry = &corpus[mutator.rng().gen_range(0..corpus.len())];
                mutator.mutate(entry, corpus)
            }
        };
        let began = Instant::now();
        let (outcome, allocation) =
            measure(|| catch_unwind(AssertUnwindSafe(|| entry_point(&input))));
        let took = began.elapsed();

        let (largest_limit, peak_limit) = allocation_limit(input.len());
        let failure = match outcome {
            Err(payload) => Some(format!(
                "panicked: {}",
                payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default()
            )),
            Ok(_) if took > budget.per_case => Some(format!("took {took:?}")),
            Ok(_) if allocation.largest > largest_limit => Some(format!(
                "allocated {} bytes at once for {} bytes of input",
                allocation.largest,
                input.len()
            )),
            Ok(_) if allocation.peak > peak_limit => Some(format!(
                "held {} bytes for {} bytes of input",
                allocation.peak,
                input.len()
            )),
            Ok(accepted) => {
                report.accepted += accepted as u64;
                None
            }
        };
        if let Some(failure) = failure {
            if let Ok(dir) = std::env::var("PEER_INPUT_FUZZ_OUT") {
                let path = std::path::Path::new(&dir).join(format!("{name}-{seed:x}-{index}.bin"));
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(&path, &input);
                eprintln!("{name}: saved the failing input to {}", path.display());
            }
            panic!("{name}: case {index} (seed {:#x}) {failure}", budget.seed);
        }
        report.cases += 1;
        report.largest_allocation = report.largest_allocation.max(allocation.largest);
        report.peak_allocation = report.peak_allocation.max(allocation.peak);
        report.slowest_case = report.slowest_case.max(took);
        index += 1;
    }
    eprintln!(
        "{name}: {} cases, {} accepted, largest allocation {} B, peak {} B, slowest {:?}",
        report.cases,
        report.accepted,
        report.largest_allocation,
        report.peak_allocation,
        report.slowest_case
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moduli_are_the_pallas_fields() {
        // 0x40000000000000000000000000000000224698fc094cf91b992d30ed00000001
        assert_eq!(FP_MODULUS[0], 1);
        assert_eq!(FP_MODULUS[31], 0x40);
        assert_eq!(&FP_MODULUS[8..16], &0x224698fc094cf91bu64.to_le_bytes());
        assert_ne!(FP_MODULUS, FQ_MODULUS);
    }

    #[test]
    fn mutants_differ_and_are_repeatable() {
        let input = vec![7u8; 100];
        let (mut a, mut b) = (Mutator::new(1), Mutator::new(1));
        let mutants: Vec<_> = (0..50).map(|_| a.mutate(&input, &[])).collect();
        assert!(mutants.iter().any(|m| m != &input));
        assert!(mutants.iter().all(|m| m == &b.mutate(&input, &[])));
    }
}
