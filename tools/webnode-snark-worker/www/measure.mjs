// P5.3 measurement: the web node's isolated prover, one instance per
// configuration (block circuits, transaction circuits, or both), proving
// OCaml's public Mesa devnet fixtures one at a time, as the bridge sends
// them: setup time and linear memory per phase, each request's round trip,
// and the instance's linear memory, sampled every 250 ms and after each
// proof. Requests and responses are saved for native verification. No key
// but the public genesis winner's is used (for the genesis transition).
import init, * as webnode from "/pkg/mina_node_web.js";

const params = new URLSearchParams(location.search);
const threads = Number(params.get("threads") ?? 8);
const network = params.get("network") ?? "devnet";
const configs = (params.get("configs") ?? "work,both,block").split(",");
// Each configuration's plan, this many times over: memory growth per request.
const repeat = Number(params.get("repeat") ?? 1);
const host = await import(params.get("snippet"));
// The public genesis winner of Mina's ledgers: a public key, no account
// needed for proving.
const PROVER = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
const FEE = "0.01";

const H = (window.harness = {
  params: { threads, network, configs, repeat },
  startedAt: Date.now(),
  runs: [],
  events: [],
  memory: [],
  done: false,
});
const report = text => {
  document.querySelector("#status").textContent = text;
  console.log(text);
};

const fixture = async name =>
  new Uint8Array(await (await fetch(`/fixtures/${name}`)).arrayBuffer());
const F = {
  base: await fixture("mesa_devnet_payment_base_job.bin"),
  left: await fixture("mesa_devnet_merge_of_bases_left_base_job.bin"),
  right: await fixture("mesa_devnet_merge_of_bases_right_base_job.bin"),
  merge: await fixture("mesa_devnet_merge_of_bases_job.bin"),
  genesis: await fixture("mesa_devnet_offline_chain_genesis_input.bin"),
};
// What each configuration proves, in order.
const PLANS = {
  work: ["base", "merge", "base+base", "merge+merge", "base"],
  both: ["block", "base", "merge", "base+base", "block", "merge+merge"],
  block: ["block", "block"],
};
const JOBS = {
  base: ["base", F.base],
  merge: ["merge", F.merge],
  "base+base": ["base", F.left, "base", F.right],
  "merge+merge": ["merge", F.merge, "merge", F.merge],
};

function save(name, bytes) {
  window.saveArtifact?.(name, Array.from(bytes));
}

globalThis.__minaBlockProverObserver = event => {
  const { request, response, report: setup, ...rest } = event;
  H.events.push({ at: Date.now(), ...rest, setup });
};

let current;
function sample() {
  if (!current) return;
  const frame = globalThis.__minaBlockProverHosts?.get(current.generation)?.frame;
  const bytes = frame?.contentWindow?.blockProverMemory?.buffer.byteLength;
  if (!bytes) return;
  current.peakLinearBytes = Math.max(current.peakLinearBytes ?? 0, bytes);
  const last = H.memory.at(-1);
  if (!last || last.bytes !== bytes || Date.now() - last.at >= 5000)
    H.memory.push({ at: Date.now(), generation: current.generation, bytes });
}
setInterval(sample, 250);

(async () => {
  if (!crossOriginIsolated) throw new Error("COOP/COEP required");
  await init();
  H.buildEnv = webnode.build_env();
  const glue = new URL("/pkg/mina_node_web.js", location.href).href;
  let generation = 0;
  for (const config of configs) {
    generation++;
    const block = config !== "work";
    const work = config !== "block";
    const run = (current = { config, generation, block, work, proofs: [] });
    H.runs.push(run);
    report(`${config}: setting up instance ${generation}`);
    const started = performance.now();
    const failures = [];
    const key = block ? webnode.prover_bridge_genesis_key_message() : null;
    run.setup = JSON.parse(
      await host.startBlockProverHost(
        glue,
        generation,
        network,
        threads,
        key,
        block,
        work,
        (g, reason) => failures.push({ g, reason, at: Date.now() }),
      ),
    );
    run.setupMs = performance.now() - started;
    run.failures = failures;
    let id = 0;
    for (const what of Array(repeat).fill(PLANS[config]).flat()) {
      id++;
      sample();
      const before = performance.now();
      let request;
      let requestBytes;
      let answer;
      if (what === "block") {
        const deadline = Date.now() + 20 * 60 * 1000;
        request = webnode.prover_bridge_block_request(id, F.genesis, true, deadline);
        save(`${config}-request-${id}-block.bin`, request);
        requestBytes = request.byteLength; // before the buffer is transferred
        answer = await host.proveWithBlockProverHost(generation, id, deadline, true, request);
      } else {
        const [k1, b1, k2, b2] = JOBS[what];
        request = webnode.prover_bridge_work_request_from_jobs(id, k1, b1, k2, b2, FEE, PROVER);
        save(`${config}-request-${id}-work.bin`, request);
        requestBytes = request.byteLength; // before the buffer is transferred
        answer = await host.proveWorkWithBlockProverHost(generation, id, 600000, request);
      }
      const ms = performance.now() - before;
      save(`${config}-response-${id}-${what === "block" ? "block" : "work"}.bin`, answer);
      sample();
      const end = H.events.findLast(
        e => (e.type === "prove-end" || e.type === "work-end") && e.generation === generation && e.requestId === id,
      );
      run.proofs.push({
        id,
        what,
        ms,
        hostMs: end?.ms,
        linearBytesAfter: end?.linearBytes,
        requestBytes,
        responseBytes: answer.byteLength,
      });
      report(`${config}: ${what} in ${(ms / 1000).toFixed(1)} s`);
    }
    run.workersBefore = await window.cdpWorkers?.();
    await host.retireBlockProverHost(generation, "measured");
    current = undefined;
  }
  H.done = true;
  report("Done");
})().catch(error => {
  H.error = String(error?.stack ?? error);
  report(H.error);
});
