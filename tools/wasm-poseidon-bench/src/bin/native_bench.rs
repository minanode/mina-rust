//! Runs the R8 kernels natively and prints ns per operation (median of 7).
//!
//! cargo run --release --bin native-bench

use std::time::Instant;

use wasm_poseidon_bench::{check, KINDS};

fn main() {
    let failures = check();
    println!("check: {failures} mismatches");
    assert_eq!(failures, 0);

    println!("{:<40} {:>12} {:>10}", "kernel", "ns/op", "iters");
    for (name, f) in KINDS {
        // Calibrate to about 100 ms per run.
        let mut n = 1000u32;
        loop {
            let t = Instant::now();
            std::hint::black_box(f(std::hint::black_box(n)));
            let dt = t.elapsed().as_secs_f64();
            if dt > 0.1 || n > u32::MAX / 4 {
                n = ((n as f64) * 0.1 / dt).max(1.0) as u32;
                break;
            }
            n *= 4;
        }
        let mut samples: Vec<f64> = (0..7)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(f(std::hint::black_box(n)));
                t.elapsed().as_secs_f64() * 1e9 / n as f64
            })
            .collect();
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{:<40} {:>12.1} {:>10}", name, samples[3], n);
    }
}
