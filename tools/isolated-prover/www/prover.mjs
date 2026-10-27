import "./workers.mjs";
// A separate Window owns stock wasm_thread's workers and HTTP dispatcher.
// The node's WebAssembly.Memory is never passed to this realm.
import init, {
  setup,
  prove,
  prove_block,
  memory_stats,
  isolation_tag,
} from "./pkg/mina_isolated_prover.js";

const clock = () => performance.timeOrigin + performance.now();
const send = (value, transfer = []) =>
  parent.postMessage(value, location.origin, transfer);
let busy = false;
let ready = false;
let wasm;
let proofKind;
let dead = false;
const MAX_BYTES = 1024 * 1024;
function fatal(reason) {
  if (dead) return;
  dead = true;
  send({ op: "host-failed", error: reason });
}
addEventListener("prover-worker-failed", event => fatal(event.detail));
addEventListener("error", event => fatal(`host error: ${event.message}`));
addEventListener("unhandledrejection", event =>
  fatal(`host rejection: ${event.reason}`),
);
addEventListener("pagehide", () => fatal("host unloaded"));

addEventListener("message", async event => {
  if (event.origin !== location.origin || event.source !== parent) return;
  const { id, op, buffer, sentAt, threads, kind = "transaction" } = event.data;
  if (dead) return send({ id, error: "prover host failed" });
  if (busy) return send({ id, error: "prover busy" });
  busy = true;
  try {
    if (op === "setup") {
      if (ready) throw new Error("already initialized");
      const start = performance.now();
      wasm = await init();
      if (isolation_tag(222) !== 0)
        throw new Error("prover memory is not fresh");
      window.proverMemory = wasm.memory; // Observation only; never an input to the node.
      window.proverStats = memory_stats;
      const instantiateMs = performance.now() - start;
      const setupStart = performance.now();
      const phases = JSON.parse(await setup(threads, kind));
      proofKind = kind;
      ready = true;
      send({
        id,
        instantiateMs,
        setupMs: performance.now() - setupStart,
        phases,
        linearBytes: wasm.memory.buffer.byteLength,
        stats: JSON.parse(memory_stats()),
        resources: performance.getEntriesByType("resource").map(r => ({
          name: new URL(r.name).pathname,
          bytes: r.decodedBodySize,
          durationMs: r.duration,
        })),
      });
    } else if (op === "prove" || op === "prove-block") {
      if (!ready) throw new Error("setup required");
      if ((op === "prove-block") !== (proofKind === "block"))
        throw new Error("proof kind differs from setup");
      if (!(buffer instanceof ArrayBuffer) || buffer.byteLength > MAX_BYTES)
        throw new Error("input exceeds 1 MiB limit or is not an ArrayBuffer");
      const receivedAt = clock();
      const start = performance.now();
      const pending = (op === "prove-block" ? prove_block : prove)(
        new Uint8Array(buffer),
      );
      const intoWasmMs = performance.now() - start;
      const result = await pending;
      const phases = JSON.parse(result.metrics());
      const copyStart = performance.now();
      const proof = result.take_proof();
      const outOfWasmMs = performance.now() - copyStart;
      result.free();
      if (proof.byteLength > MAX_BYTES)
        throw new Error("proof exceeds 1 MiB limit");
      send(
        {
          id,
          proof: proof.buffer,
          phases,
          intoWasmMs,
          outOfWasmMs,
          inputTransferMs: receivedAt - sentAt,
          linearBytes: wasm.memory.buffer.byteLength,
          stats: JSON.parse(memory_stats()),
          sentAt: clock(),
        },
        [proof.buffer],
      );
    } else {
      throw new Error(`unknown operation: ${op}`);
    }
  } catch (error) {
    send({ id, error: String(error.stack ?? error) });
  } finally {
    busy = false;
  }
});
send({ op: "host-ready" });
