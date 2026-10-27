// I34c harness: a web node, alone on a private Mesa chain with a throwaway
// producer key, proves its genesis and its own blocks in its isolated prover
// iframe. With fault=1, the iframe is removed in the middle of the proof of
// block blocks+1 (the reviewer's fault test). The producer retries a failed
// proof once (P15), on the replacement prover; with faults=2 that retry's
// iframe is removed too, in the middle of its proof, so the slot must be
// discarded. The harness then waits until the faulted slot is produced or
// discarded and recoveryBlocks later blocks are produced.
//
// Records lifecycle events and the bridge's request and response bytes (no
// key ever crosses the bridge), the node's and each prover's linear memory
// every 250 ms, node status every 5 s, and CDP worker targets.
import init, * as webnode from "/pkg/mina_node_web.js";

const params = new URLSearchParams(location.search);
const threads = Number(params.get("threads") ?? 8);
const blocksBeforeFault = Number(params.get("blocks") ?? 2);
const fault = params.get("fault") !== "0";
const faultPhase = params.get("faultPhase") ?? "block";
const faultKind = params.get("faultKind") ?? "death";
const faultDelayMs = Number(params.get("faultDelayMs") ?? 15000);
const faults = Number(params.get("faults") ?? 1);
const retryFaultDelayMs = Number(params.get("retryFaultDelayMs") ?? 10000);
const recoveryBlocks = Number(params.get("recoveryBlocks") ?? 1);
globalThis.MINA_BLOCK_PROVER_THREADS = threads;

// The only peer: nothing listens there. It keeps the node from its compiled
// default peer; the runner's Chrome flags block any other network too.
const DEAD_PEER =
  "/2bjYBqn45MmtismsAYP9rZ6Xns9snCcNsN1eDgQZB5s6AzY2CR2/http/127.0.0.1/9";
const relay = params.get("relay") ?? DEAD_PEER;
if (params.has("relay") && !/^\/ip4\/127\.0\.0\.1\/tcp\/\d+\/webrtc\/http\/p2p\/[A-Za-z0-9]+$/.test(relay))
  throw new Error("relay must be a loopback WebRTC address");
const PRODUCED = ["BlockApplySuccess", "Committed", "Canonical"];

const H = (window.harness = {
  params: {
    threads,
    blocksBeforeFault,
    fault,
    faultPhase,
    faultKind,
    faults,
    faultDelayMs,
    retryFaultDelayMs,
    recoveryBlocks,
  },
  faults: [],
  startedAt: Date.now(),
  events: [],
  memory: [],
  status: [],
  workers: [],
  peak: { node: 0, prover: {} },
  done: false,
  phase: "loading",
});
const report = text => {
  document.querySelector("#status").textContent = text;
  console.log(text);
};

let nodeMemory;
function sampleMemory() {
  const at = Date.now();
  const node = nodeMemory?.buffer.byteLength ?? 0;
  const prover = {};
  for (const [generation, host] of globalThis.__minaBlockProverHosts ?? []) {
    // Through contentWindow only: no reference that outlives the iframe.
    const memory = host.frame.contentWindow?.blockProverMemory;
    if (memory) prover[generation] = memory.buffer.byteLength;
  }
  H.peak.node = Math.max(H.peak.node, node);
  for (const [generation, bytes] of Object.entries(prover))
    H.peak.prover[generation] = Math.max(H.peak.prover[generation] ?? 0, bytes);
  const last = H.memory.at(-1);
  const proverJson = JSON.stringify(prover);
  if (
    !last ||
    at - last.at >= 1000 ||
    last.node !== node ||
    JSON.stringify(last.prover) !== proverJson
  )
    H.memory.push({ at, node, prover });
}
setInterval(sampleMemory, 250);

function save(name, bytes) {
  window.saveArtifact?.(name, Array.from(bytes));
}

let blockProofs = 0;
globalThis.__minaBlockProverObserver = event => {
  const { request, response, ...rest } = event;
  const entry = { at: Date.now(), ...rest };
  if (request) {
    entry.requestBytes = request.byteLength;
    entry.isGenesis = event.isGenesis;
    save(`request-${event.generation}-${event.requestId}.bin`, request);
  }
  if (response) {
    entry.responseBytes = response.byteLength;
    save(`response-${event.generation}-${event.requestId}.bin`, response);
  }
  H.events.push(entry);
  console.log(`prover ${event.type} ${JSON.stringify(rest).slice(0, 400)}`);
  if (event.type === "prove-start" && (faultPhase === "genesis" ? entry.isGenesis : !entry.isGenesis)) {
    blockProofs++;
    const first = H.faults[0];
    if (fault && !first && blockProofs === (faultPhase === "genesis" ? 1 : blocksBeforeFault + 1))
      scheduleFault(event, faultDelayMs);
    // The retry of the faulted slot: same deadline, the replacement prover.
    else if (
      faults >= 2 &&
      first &&
      H.faults.length === 1 &&
      (event.generation > first.generation || faultKind === "proof-error")
    ) {
      if (event.deadlineMs === first.deadlineMs)
        scheduleFault(event, retryFaultDelayMs);
      else H.retryFaultSkipped = "the replacement's first proof was another slot's";
    }
  }
};

function scheduleFault(event, delayMs) {
  const info = {
    generation: event.generation,
    requestId: event.requestId,
    deadlineMs: event.deadlineMs,
    proveStartAt: Date.now(),
    workerChecks: [],
  };
  H.faults.push(info);
  H.fault = H.faults[0];
  setTimeout(async () => {
    const host = globalThis.__minaBlockProverHosts?.get(event.generation);
    if (!host || host.dead) {
      info.skipped = "the host was already gone";
      return;
    }
    info.workersBefore = await window.cdpWorkers?.();
    info.ownedWorkersBefore = host.frame.contentWindow?.blockProverWorkerCounts?.();
    info.pendingBefore = host.pending.size;
    info.removedAt = Date.now();
    if (faultKind === "proof-error") {
      // Arm a normal request failure while proving, delivered when the
      // worker completes. This does not start two expensive proofs at once.
      for (const item of host.pending.values()) {
        item.resolve = () => item.reject(new Error("injected genesis proof failure"));
      }
    } else {
      host.frame.remove();
    }
    for (const delay of [0, 1000, 2000, 5000, 10000, 30000])
      setTimeout(async () => {
        info.workerChecks.push({
          afterMs: Date.now() - info.removedAt,
          workers: await window.cdpWorkers?.(),
        });
      }, delay);
  }, delayMs);
}

// Mesa's block window: a request's deadline is the end of its slot.
const BLOCK_WINDOW_MS = 90000;

function finished(attempts) {
  const produced = attempts.filter(a => PRODUCED.includes(a.status));
  if (!fault) return produced.length >= blocksBeforeFault;
  const last = H.faults.at(-1);
  if (!last?.removedAt) return false;
  if (faults >= 2 && H.faults.length < 2 && !H.retryFaultSkipped) return false;
  if (faultPhase === "genesis") {
    H.faultOutcome = { genesis: H.genesisState, produced: produced.length };
    if (H.genesisState === "ProveError") {
      H.terminalObservedAt ??= Date.now();
      return Date.now() - H.terminalObservedAt >= 30_000;
    }
    return H.genesisState === "ProveSuccess" && produced.length >= recoveryBlocks;
  }
  const slotMs = H.faults[0].deadlineMs - BLOCK_WINDOW_MS;
  const faulted = attempts.find(
    a => Math.abs(a.won_slot.slot_time / 1e6 - slotMs) < 1000,
  );
  const after = produced.filter(a => a.won_slot.slot_time / 1e6 > last.removedAt);
  H.faultOutcome = {
    faultedSlot: faulted?.won_slot.global_slot,
    faultedStatus: faulted?.status,
    discardReason: faulted?.discard_reason,
    discardedAt: faulted?.times?.discarded,
    producedAfter: after.length,
  };
  const settled =
    faulted &&
    (faulted.status === "Discarded" || PRODUCED.includes(faulted.status));
  return settled && after.length >= recoveryBlocks;
}

(async () => {
  if (!crossOriginIsolated) throw new Error("COOP/COEP required");
  const wasm = await init();
  H.buildEnv = webnode.build_env();
  nodeMemory = wasm.memory;
  H.nodeMemoryAtInit = nodeMemory.buffer.byteLength;
  const key = await (await fetch("/run/producer.json")).json();
  report("Starting the web node; its prover iframe loads the block circuits now");
  H.phase = "starting";
  const rpc = await webnode.run(
    [key.encrypted, key.password],
    null,
    [relay],
    new URL("/run/genesis.bin", location.href).href,
    "mainnet",
  );
  H.nodeStartedAt = Date.now();
  H.phase = "running";
  window.rpc = rpc;
  while (!H.done) {
    await new Promise(resolve => setTimeout(resolve, 5000));
    const status = await rpc.status();
    const producer = await rpc.stats().block_producer();
    const pool = await rpc.transaction_pool().get();
    const attempts = producer?.attempts ?? [];
    if (faultPhase === "genesis") {
      const genesis = await rpc.state().get("$.transition_frontier.genesis");
      H.genesisState = typeof genesis === "string" ? genesis : Object.keys(genesis ?? {})[0];
      H.genesisSnapshot = genesis;
    }
    H.attempts = attempts;
    H.chainId = status?.chain_id;
    H.peers = status?.peers;
    H.status.push({
      at: Date.now(),
      bestTip: status?.transition_frontier?.best_tip,
      sync: status?.transition_frontier?.sync?.status,
      attempt: status?.current_block_production_attempt?.status,
      queues: status?.service_queues,
      peers: status?.peers?.length ?? 0,
      connectedPeers:
        status?.peers?.filter(p => p.connection_status === "Connected").length ?? 0,
      poolSize: pool?.length ?? 0,
    });
    H.workers.push({ at: Date.now(), ...((await window.cdpWorkers?.()) ?? {}) });
    const produced = attempts.filter(a => PRODUCED.includes(a.status)).length;
    report(
      `best tip ${JSON.stringify(status?.transition_frontier?.best_tip)}; ` +
        `produced ${produced}; attempt ${status?.current_block_production_attempt?.status}`,
    );
    if (finished(attempts)) {
      H.phase = "settling";
      await new Promise(resolve => setTimeout(resolve, 20000));
      H.finalStatus = await rpc.status();
      H.finalAccounts = await rpc.ledger().latest().accounts().all();
      H.done = true;
    }
  }
  report("Done");
})().catch(error => {
  H.error = String(error?.stack ?? error);
  report(H.error);
});
