// P5.3 gate: the web node in headless Chrome as the private network's SNARK
// worker. It syncs from its only peer, our native relay, which gossips its
// work to an OCaml 4.0.2 block producer. The worker's public key and fee
// come from /run/worker.json; no private key is in this page.
//
// Records, for the runner: the isolated prover's lifecycle and every work
// request and response (saved as artifacts for native verification), the
// node's and the prover's linear memory every second, and every 10 s the
// node's status, its SNARK worker state and its pool's admission counts.
import init, * as webnode from "/pkg/mina_node_web.js";

const params = new URLSearchParams(location.search);
const relay = params.get("relay");
if (!/^\/ip4\/127\.0\.0\.1\/tcp\/\d+\/webrtc\/http\/p2p\/[A-Za-z0-9]+$/.test(relay ?? ""))
  throw new Error("relay must be a loopback WebRTC address");
const threads = Number(params.get("threads") ?? 8);
globalThis.MINA_BLOCK_PROVER_THREADS = threads;

const H = (window.harness = {
  params: { relay, threads },
  startedAt: Date.now(),
  events: [],
  memory: [],
  status: [],
  peak: { node: 0, prover: {} },
  phase: "loading",
});
const report = text => {
  document.querySelector("#status").textContent = text;
  console.log(text);
};
const save = (name, bytes) => window.saveArtifact?.(name, Array.from(bytes));

globalThis.__minaBlockProverObserver = event => {
  const { request, response, ...rest } = event;
  const entry = { at: Date.now(), ...rest };
  if (request) {
    entry.requestBytes = request.byteLength;
    save(`work-request-${event.generation}-${event.requestId}.bin`, request);
  }
  if (response) {
    entry.responseBytes = response.byteLength;
    save(`work-response-${event.generation}-${event.requestId}.bin`, response);
  }
  H.events.push(entry);
  console.log(`prover ${event.type} ${JSON.stringify(rest).slice(0, 300)}`);
};

let nodeMemory;
function sampleMemory() {
  const node = nodeMemory?.buffer.byteLength ?? 0;
  const prover = {};
  for (const [generation, host] of globalThis.__minaBlockProverHosts ?? []) {
    const memory = host.frame?.contentWindow?.blockProverMemory;
    if (memory) prover[generation] = memory.buffer.byteLength;
  }
  H.peak.node = Math.max(H.peak.node, node);
  for (const [generation, bytes] of Object.entries(prover))
    H.peak.prover[generation] = Math.max(H.peak.prover[generation] ?? 0, bytes);
  const last = H.memory.at(-1);
  if (!last || last.node !== node || JSON.stringify(last.prover) !== JSON.stringify(prover) ||
      Date.now() - last.at >= 60000)
    H.memory.push({ at: Date.now(), node, prover });
}
setInterval(sampleMemory, 1000);

(async () => {
  if (!crossOriginIsolated) throw new Error("COOP/COEP required");
  const wasm = await init();
  H.buildEnv = webnode.build_env();
  nodeMemory = wasm.memory;
  const worker = await (await fetch("/run/worker.json")).json();
  H.worker = { publicKey: worker.publicKey, fee: worker.fee };
  report("Starting the web node as a SNARK worker");
  H.phase = "starting";
  const rpc = await webnode.run(
    null,
    null,
    [relay],
    new URL("/run/genesis.bin", location.href).href,
    "devnet",
    null,
    worker.publicKey,
    worker.fee,
  );
  window.rpc = rpc;
  H.nodeStartedAt = Date.now();
  H.phase = "running";
  for (;;) {
    await new Promise(resolve => setTimeout(resolve, 10000));
    const status = await rpc.status();
    const snarkWorker = await rpc.state().get("$.external_snark_worker");
    const admission = await rpc.state().get("$.snark_pool.admission_stats");
    const paused = await rpc.state().get("$.snark_pool.worker_paused_for_block");
    H.status.push({
      at: Date.now(),
      bestTip: status?.transition_frontier?.best_tip,
      sync: status?.transition_frontier?.sync?.status,
      peers: status?.peers?.filter(p => p.connection_status === "Connected").length ?? 0,
      snarkWorker,
      admission,
      paused,
    });
    const done = H.events.filter(e => e.type === "work-end").length;
    report(`best tip ${JSON.stringify(status?.transition_frontier?.best_tip)}; bundles proved ${done}`);
  }
})().catch(error => {
  H.error = String(error?.stack ?? error);
  report(H.error);
});
