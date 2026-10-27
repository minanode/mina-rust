// Runs the r8_* kernels of a wasm-poseidon-bench module and returns ns per
// operation, median of `reps` runs of about `targetMs` each. Shared by the
// browser page (index.html) and Node (run-node.mjs).

export async function runBench(
  bytes,
  { targetMs = 150, reps = 7, filter = null, log = () => {} } = {},
) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const ex = instance.exports;
  const t0 = performance.now();
  const mismatches = ex.r8_check();
  const checkMs = performance.now() - t0;
  log(`check: ${mismatches} mismatches (${checkMs.toFixed(0)} ms)`);
  const decoder = new TextDecoder();
  const results = [];
  for (let k = 0; k < ex.r8_kinds(); k++) {
    const name = decoder.decode(
      new Uint8Array(ex.memory.buffer, ex.r8_name_ptr(k), ex.r8_name_len(k)),
    );
    if (filter && !filter.test(name)) continue;
    // Calibrate, which also lets V8 tier the function up to TurboFan.
    let n = 64;
    for (;;) {
      const t = performance.now();
      ex.r8_bench(k, n);
      const dt = performance.now() - t;
      if (dt > targetMs / 4 || n > 2 ** 30) {
        n = Math.max(1, Math.round((n * targetMs) / Math.max(dt, 0.01)));
        break;
      }
      n *= 4;
    }
    // Warm up at full size for about half a second, then give background
    // TurboFan compilation a moment to land.
    for (const t = performance.now(); performance.now() - t < 500; )
      ex.r8_bench(k, n);
    await new Promise(r => setTimeout(r, 20));
    const samples = [];
    let sink = 0;
    for (let r = 0; r < reps; r++) {
      const t = performance.now();
      sink ^= ex.r8_bench(k, n);
      samples.push(((performance.now() - t) * 1e6) / n);
    }
    samples.sort((a, b) => a - b);
    const nsPerOp = samples[Math.floor(reps / 2)];
    results.push({
      name,
      nsPerOp,
      min: samples[0],
      max: samples[reps - 1],
      n,
      sink,
    });
    log(
      `${name.padEnd(40)} ${nsPerOp.toFixed(1).padStart(12)} ns/op  (min ${samples[0].toFixed(1)}, max ${samples[reps - 1].toFixed(1)}, n=${n})`,
    );
  }
  return { mismatches, results };
}
