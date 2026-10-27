import "./save-proof.mjs";
import { ProverHost, MAX_BYTES } from "./client.mjs";
import init, {
  memory_stats,
  isolation_tag,
} from "./pkg/mina_isolated_prover.js";

const clock = () => performance.timeOrigin + performance.now();
const params = new URLSearchParams(location.search);
const threads = Number(params.get("threads") ?? 8);
const runs = Number(params.get("runs") ?? 2);
const generations = Number(params.get("generations") ?? 1);
const kind = params.get("kind") ?? "transaction";
const op = kind === "block" ? "prove-block" : "prove";
const report = text => {
  document.querySelector("pre").textContent = text;
  console.log(text);
};
window.runResult = null;
window.runError = null;
window.teardownComplete = false;
window.runProgress = { phase: "loading", linearPeakBytes: 0 };

async function rejected(promise, expected) {
  try {
    await promise;
  } catch (error) {
    if (!String(error).includes(expected)) throw error;
    return String(error);
  }
  throw new Error(`expected rejection: ${expected}`);
}

// Exercise real browser lifecycle events with requests pending. No private data.
async function deathControls() {
  const results = [];
  for (const mode of ["iframe", "worker"]) {
    const host = new ProverHost();
    try {
      await host.ready;
      // Do not deliver this probe: keep a request pending until the fault occurs.
      // The normal request bookkeeping, error listener and cleanup are exercised.
      const started = performance.now();
      const id = ++host.nextId;
      const promise = new Promise((resolve, reject) => {
        host.pending.set(id, {
          resolve,
          reject,
          timer: setTimeout(
            () => reject(new Error("death control timeout")),
            3000,
          ),
        });
      });
      const check = rejected(
        promise,
        mode === "iframe" ? "iframe removed" : "worker error",
      );
      if (mode === "iframe") host.frame.remove();
      else {
        const realm = host.frame.contentWindow;
        const url = realm.URL.createObjectURL(
          new realm.Blob(['throw new Error("I34b worker death control")'], {
            type: "text/javascript",
          }),
        );
        new realm.Worker(url);
        // The URL belongs to the disposable realm, which retirement discards.
      }
      const error = await check;
      const rejectedMs = performance.now() - started;
      if (rejectedMs >= 3000 || host.pending.size)
        throw new Error("pending work survived host death");
      results.push({
        mode,
        rejectedMs,
        error,
        pendingAfter: host.pending.size,
      });
    } finally {
      await host.retire();
    }
  }
  return results;
}

(async () => {
  if (!crossOriginIsolated) throw new Error("COOP/COEP required");
  const node = await init();
  if (isolation_tag(111) !== 0) throw new Error("node memory is not fresh");
  const nodeBaseline = node.memory.buffer.byteLength;
  const nodeStatsBefore = JSON.parse(memory_stats());
  const response = await fetch(
    kind === "block" ? "/block-fixture.bin" : "/fixture.bin",
  );
  if (!response.ok) throw new Error(`fixture HTTP ${response.status}`);
  const fixture = await response.arrayBuffer();
  const instances = [];
  let heartbeats = 0,
    maxHeartbeatGapMs = 0,
    lastBeat = performance.now();
  let host;
  const samples = [];
  const timer = setInterval(() => {
    const now = performance.now();
    maxHeartbeatGapMs = Math.max(maxHeartbeatGapMs, now - lastBeat);
    lastBeat = now;
    heartbeats++;
    const memory = host?.frame.contentWindow?.proverMemory;
    if (memory) {
      const linearBytes = memory.buffer.byteLength;
      samples.push({ atMs: now, generation: instances.length, linearBytes });
      window.runProgress.linearPeakBytes = Math.max(
        window.runProgress.linearPeakBytes,
        linearBytes,
      );
    }
  }, 100);
  let proofNumber = 0;
  try {
    for (let generation = 0; generation < generations; generation++) {
      host = new ProverHost(Number(params.get("timeoutMs") ?? 900000));
      const instance = { generation, results: [] };
      try {
        await host.ready;
        report(
          `Generation ${generation}: loading ${kind} prover SRS, verifiers and indexes…`,
        );
        window.runProgress.phase = `setup-${generation}`;
        instance.setup = await host.request("setup", { threads, kind });
        window.runProgress.setup = instance.setup;
        if (isolation_tag(111) !== 111)
          throw new Error("WASM statics alias across realms");
        if (node.memory === host.frame.contentWindow.proverMemory)
          throw new Error("memories alias");
        instance.malformedRejected = await rejected(
          host.request(op, { buffer: new ArrayBuffer(0), sentAt: clock() }),
          "failed to fill whole buffer",
        );
        const trailing = new Uint8Array(fixture.byteLength + 1);
        trailing.set(new Uint8Array(fixture));
        instance.trailingRejected = await rejected(
          host.request(op, { buffer: trailing.buffer, sentAt: clock() }, [
            trailing.buffer,
          ]),
          "trailing bytes",
        );
        const oversized = new ArrayBuffer(MAX_BYTES + 1);
        instance.oversizedRejected = await rejected(
          host.request(op, { buffer: oversized }, [oversized]),
          "1 MiB",
        );
        if (oversized.byteLength !== MAX_BYTES + 1)
          throw new Error("oversized input transferred");
        // Bypass the client to check the receiving realm's independent byte limit.
        const directId = ++host.nextId;
        const receiverCheck = new Promise((resolve, reject) => {
          host.pending.set(directId, {
            resolve,
            reject,
            timer: setTimeout(
              () => reject(new Error("receiver limit timeout")),
              3000,
            ),
          });
        });
        const receiverRejected = rejected(receiverCheck, "1 MiB");
        host.frame.contentWindow.postMessage(
          { id: directId, op, buffer: oversized },
          location.origin,
          [oversized],
        );
        instance.receiverOversizedRejected = await receiverRejected;
        const count = generation === 0 ? runs : 1;
        for (let n = 0; n < count; n++) {
          report(
            `Generation ${generation}: proving Mesa ${kind} ${n + 1}/${count}…`,
          );
          window.runProgress.phase = `proof-${generation}-${n}`;
          const buffer = fixture.slice(0);
          const start = performance.now();
          const task = host.request(op, { buffer, sentAt: clock() }, [buffer]);
          const postMessageMs = performance.now() - start;
          if (buffer.byteLength !== 0)
            throw new Error("input transfer did not detach");
          const result = await task;
          result.roundTripMs = performance.now() - start;
          result.postMessageMs = postMessageMs;
          result.inputBytes = fixture.byteLength;
          result.outputBytes = result.proof.byteLength;
          result.proofNumber = proofNumber++;
          await window.saveProof(
            result.proofNumber,
            Array.from(new Uint8Array(result.proof)),
          );
          delete result.proof;
          instance.results.push(result);
          window.runProgress.completedProofs = proofNumber;
        }
        instance.finalLinearBytes =
          host.frame.contentWindow.proverMemory.buffer.byteLength;
      } finally {
        instance.terminatedWorkers = await host.retire();
        host = null;
      }
      if (node.memory.buffer.byteLength !== nodeBaseline)
        throw new Error("parent linear memory grew");
      if (window.workersRemaining) {
        instance.workersAfterRetirement = await window.workersRemaining();
        if (instance.workersAfterRetirement !== 0)
          throw new Error("workers survived retirement");
      }
      instances.push(instance);
    }
    const controls = await deathControls();
    window.runResult = {
      kind,
      threads,
      runs,
      generations,
      proofCount: proofNumber,
      instances,
      setup: instances[0].setup,
      results: instances.flatMap(i => i.results),
      deathControls: controls,
      nodeBaseline,
      nodeFinal: node.memory.buffer.byteLength,
      nodeStatsBefore,
      nodeStatsAfter: JSON.parse(memory_stats()),
      maxHeartbeatGapMs,
      heartbeats,
      samples,
      isolation:
        "fresh independent WASM statics per generation; transferred inputs detached; parent memory unchanged",
    };
    report("Proofs returned; native verification follows in the runner.");
    window.runProgress.phase = "returned";
  } finally {
    clearInterval(timer);
    if (host) await host.retire();
    window.teardownComplete = true;
  }
})().catch(error => {
  window.runError = String(error.stack ?? error);
  report(window.runError);
});
