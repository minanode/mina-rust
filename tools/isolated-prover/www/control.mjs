import { stopWorkers } from "./workers.mjs";
import "./save-proof.mjs";
// Control: the same prover and pool allocate in the page's own WASM memory.
import init, {
  setup,
  prove,
  memory_stats,
} from "./pkg/mina_isolated_prover.js";
const params = new URLSearchParams(location.search);
const threads = Number(params.get("threads") ?? 8);
const runs = Number(params.get("runs") ?? 2);
window.runResult = null;
window.runError = null;
window.teardownComplete = false;
window.runProgress = { phase: "loading", linearPeakBytes: 0 };
const report = text => {
  document.querySelector("pre").textContent = text;
  console.log(text);
};
(async () => {
  const start = performance.now();
  const wasm = await init();
  const instantiateMs = performance.now() - start;
  const nodeBaseline = wasm.memory.buffer.byteLength;
  const nodeStatsBefore = JSON.parse(memory_stats());
  const samples = [];
  const timer = setInterval(() => {
    const linearBytes = wasm.memory.buffer.byteLength;
    samples.push({ atMs: performance.now(), linearBytes });
    window.runProgress.linearPeakBytes = linearBytes;
  }, 100);
  try {
    report("Control: loading prover in page memory…");
    window.runProgress.phase = "setup";
    const setupStart = performance.now();
    const phases = JSON.parse(await setup(threads, "transaction"));
    const setupResult = {
      instantiateMs,
      setupMs: performance.now() - setupStart,
      phases,
      linearBytes: wasm.memory.buffer.byteLength,
      stats: JSON.parse(memory_stats()),
    };
    const response = await fetch("/fixture.bin");
    if (!response.ok) throw new Error(`fixture HTTP ${response.status}`);
    const fixture = new Uint8Array(await response.arrayBuffer());
    const results = [];
    for (let n = 0; n < runs; n++) {
      report(`Control: proving ${n + 1}/${runs}…`);
      window.runProgress.phase = `proof-${n}`;
      const start = performance.now();
      const pending = prove(fixture);
      const intoWasmMs = performance.now() - start;
      const result = await pending;
      const phases = JSON.parse(result.metrics());
      const copyStart = performance.now();
      const proof = result.take_proof();
      const outOfWasmMs = performance.now() - copyStart;
      result.free();
      const roundTripMs = performance.now() - start;
      results.push({
        phases,
        intoWasmMs,
        outOfWasmMs,
        roundTripMs,
        inputBytes: fixture.byteLength,
        outputBytes: proof.byteLength,
        linearBytes: wasm.memory.buffer.byteLength,
        stats: JSON.parse(memory_stats()),
      });
      await window.saveProof(n, Array.from(proof));
    }
    window.runResult = {
      threads,
      runs,
      setup: setupResult,
      results,
      nodeBaseline,
      nodeFinal: wasm.memory.buffer.byteLength,
      nodeStatsBefore,
      nodeStatsAfter: JSON.parse(memory_stats()),
      samples,
      isolation: "control: prover and page use one WASM memory",
    };
    window.runProgress.phase = "returned";
    report("Control proofs returned.");
  } finally {
    clearInterval(timer);
    const terminatedWorkers = stopWorkers();
    if (window.runResult)
      window.runResult.terminatedWorkers = terminatedWorkers;
    await new Promise(resolve => setTimeout(resolve, 1000));
    window.teardownComplete = true;
  }
})().catch(error => {
  window.runError = String(error.stack ?? error);
  report(window.runError);
});
