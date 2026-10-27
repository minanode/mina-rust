// I34c: the hosts of the web node's isolated block prover.
//
// Each host is a hidden, same-origin iframe that instantiates the web node's
// own module again, with a fresh WebAssembly.Memory and its own workers, and
// proves blocks there, SNARK work (P5.3), or both: one request at a time. The node's memory is never passed to it: requests and
// proofs cross as transferred ArrayBuffers, each bounded in size on both
// sides. A host is one "generation"; the Rust bridge
// (mina_node_common::block_producer::isolated) numbers them and ignores
// answers from any but the current one.
//
// A host fails, rejecting everything pending at once, when its iframe is
// removed or navigates, when it or one of its workers throws, when a request
// misses its deadline, or when it breaks a size limit. Retiring a host
// terminates its workers while the iframe is still attached, waits a second
// (I34: removing the iframe alone left the workers alive), then removes it.
//
// Wasm-bindgen imports this file wherever the module is loaded, workers
// included: nothing here touches the DOM until a host is started.

const MAX_REQUEST_BYTES = 1024 * 1024;
const MAX_RESPONSE_BYTES = 64 * 1024;
const MAX_KEY_BYTES = 1024;
const LOAD_TIMEOUT_MS = 60 * 1000;
const SETUP_TIMEOUT_MS = 20 * 60 * 1000;
const KEY_TIMEOUT_MS = 30 * 1000;
const RETIRE_GRACE_MS = 1000;

const hosts = new Map();

function fatalError(message) {
  const error = new Error(message);
  error.fatal = true;
  return error;
}

// Optional instrumentation for tests and measurements: called with public
// data only (lifecycle events, request and response bytes; never the key).
function observe(event) {
  const observer = globalThis.__minaBlockProverObserver;
  if (typeof observer !== "function") return;
  try {
    observer(event);
  } catch (error) {
    console.warn("block prover observer failed", error);
  }
}

// The WASM's URL and SHA-256 when the page pins them (W1: a WASM on another
// host; web-node.service.ts sets them before the node starts), else null:
// the WASM next to the glue, loaded as before.
export function pinnedWasm() {
  const pin = globalThis.__MINA_WEBNODE_WASM__;
  if (!pin) return null;
  if (typeof pin.url !== "string" || !/^[0-9a-f]{64}$/.test(pin.sha256 ?? ""))
    throw fatalError("the page pins the web node's WASM badly");
  return { url: pin.url, sha256: pin.sha256 };
}

// The pinned WASM's bytes, or an error: bytes that are not the page's are
// never compiled. The realm's document embeds this function's source;
// exported for its test (frontend/tests/block-prover-host.test.mjs).
export async function verifiedWasm(pin, fetchFn = fetch) {
  const response = await fetchFn(pin.url);
  if (!response.ok) throw new Error("prover WASM " + pin.url + ": HTTP " + response.status);
  const bytes = await response.arrayBuffer();
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const hex = Array.from(digest, b => b.toString(16).padStart(2, "0")).join("");
  if (hex !== pin.sha256)
    throw new Error("prover WASM from " + pin.url + " is not the one this page was built with (SHA-256 " +
      hex + ", expected " + pin.sha256 + "): not starting the prover");
  return bytes;
}

// The iframe's document. It marks its realm, wraps Worker so that it owns
// every worker the prover starts, then imports the module: with a pinned
// WASM, from bytes it has checked against the pin.
function realmDocument(glueUrl, generation, origin, wasm) {
  const constants = JSON.stringify({
    glueUrl,
    generation,
    origin,
    wasm,
    MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES,
  }).replace(/</g, "\\u003c");
  return `<!doctype html><meta charset="utf-8"><title>Mina block prover</title>
<script type="module">
const C = ${constants};
globalThis.__MINA_BLOCK_PROVER_REALM__ = true;
const post = (message, transfer = []) =>
  parent.postMessage(message, C.origin, transfer);
let dead = false;
let stopping = false;
function fail(reason) {
  if (dead) return;
  dead = true;
  post({ op: "host-failed", reason });
}
const NativeWorker = globalThis.Worker;
const owned = new Set();
let spawned = 0;
globalThis.Worker = class extends NativeWorker {
  constructor(url, options = {}) {
    // Read each option: wasm_thread's module-worker polyfill probes support
    // with a non-enumerable "type" getter, which a spread skips, and then
    // takes over Worker with classic workers that read a module URL from
    // the first line of the name. So the tag goes at the end of the name.
    const tag = "mina-block-prover-" + C.generation;
    const name = options.name ? options.name + " " + tag : tag;
    super(url, { type: options.type, credentials: options.credentials, name });
    owned.add(this);
    spawned++;
    this.addEventListener("error", event => {
      event.preventDefault();
      if (!stopping) fail("prover worker error: " + event.message);
    });
    this.addEventListener("messageerror", () => {
      if (!stopping) fail("prover worker message error");
    });
  }
  postMessage(...args) {
    this.hasWork = true;
    super.postMessage(...args);
  }
  terminate() {
    owned.delete(this);
    super.terminate();
    // wasm_thread probes module support with an empty worker; only a
    // worker that was given work can take a request down with it.
    if (this.hasWork && !stopping) fail("prover worker terminated");
  }
};
globalThis.stopBlockProverWorkers = () => {
  stopping = true;
  const count = owned.size;
  for (const worker of owned) worker.terminate();
  owned.clear();
  return count;
};
globalThis.blockProverWorkerCounts = () => ({ owned: owned.size, spawned });
// Removing the iframe unloads this document: stop the pool first.
addEventListener("pagehide", () => {
  globalThis.stopBlockProverWorkers();
  fail("prover host unloaded");
});
addEventListener("error", event => fail("prover host error: " + event.message));
addEventListener("unhandledrejection", event =>
  fail("prover host rejection: " + event.reason),
);
${verifiedWasm}
let glue;
let ready = false;
let busy = false;
addEventListener("message", async event => {
  if (event.source !== parent) return;
  const { id, op } = event.data ?? {};
  if (dead) return post({ id, error: "prover host failed", fatal: true });
  if (busy) return post({ id, error: "prover host busy", fatal: false });
  busy = true;
  try {
    if (op === "setup") {
      const started = performance.now();
      glue = await import(C.glueUrl);
      let bytes;
      try {
        bytes = C.wasm ? await verifiedWasm(C.wasm) : undefined;
      } catch (error) {
        fail(String(error?.message ?? error));
        return post({ id, error: String(error?.message ?? error), fatal: true });
      }
      const wasm = await glue.default(bytes ? { module_or_path: bytes } : undefined);
      globalThis.blockProverMemory = wasm.memory;
      const instantiateMs = performance.now() - started;
      const report = JSON.parse(
        await glue.block_prover_realm_setup(
          event.data.network,
          event.data.threads,
          event.data.block,
          event.data.work,
        ),
      );
      report.instantiate_ms = instantiateMs;
      report.linear_bytes = wasm.memory.buffer.byteLength;
      post({ id, report: JSON.stringify(report) });
    } else if (op === "install-key") {
      const key = event.data.key;
      if (!(key instanceof ArrayBuffer) || key.byteLength > ${MAX_KEY_BYTES})
        throw new Error("invalid producer key message");
      glue.block_prover_realm_install_key(new Uint8Array(key));
      ready = true;
      post({ id, ok: true });
    } else if (op === "work-ready") {
      // A host that proves only SNARK work has no key to install.
      ready = true;
      post({ id, ok: true });
    } else if (op === "prove-work") {
      if (!ready) throw new Error("prover host is not set up");
      const buffer = event.data.buffer;
      if (!(buffer instanceof ArrayBuffer) || buffer.byteLength > C.MAX_REQUEST_BYTES)
        throw new Error("SNARK work request exceeds the size limit");
      const response = await glue.block_prover_realm_prove_work(new Uint8Array(buffer));
      if (response.byteLength > C.MAX_RESPONSE_BYTES) {
        fail("SNARK work response exceeds the size limit");
        return post({ id, error: "response exceeds the size limit", fatal: true });
      }
      const out = response.buffer.byteLength === response.byteLength
        ? response.buffer
        : response.slice().buffer;
      post({ id, buffer: out, linearBytes: wasm_bytes() }, [out]);
    } else if (op === "prove") {
      if (!ready) throw new Error("prover host is not set up");
      const buffer = event.data.buffer;
      if (!(buffer instanceof ArrayBuffer) || buffer.byteLength > C.MAX_REQUEST_BYTES)
        throw new Error("block prover request exceeds the size limit");
      // The size is checked before this copy into the prover's memory.
      const response = await glue.block_prover_realm_prove(new Uint8Array(buffer));
      if (response.byteLength > C.MAX_RESPONSE_BYTES) {
        fail("block prover response exceeds the size limit");
        return post({ id, error: "response exceeds the size limit", fatal: true });
      }
      const out = response.buffer.byteLength === response.byteLength
        ? response.buffer
        : response.slice().buffer;
      post({ id, buffer: out, linearBytes: wasm_bytes() }, [out]);
    } else {
      throw new Error("unknown prover host operation: " + op);
    }
  } catch (error) {
    post({ id, error: String(error?.message ?? error), fatal: false });
  } finally {
    busy = false;
  }
});
function wasm_bytes() {
  return globalThis.blockProverMemory?.buffer.byteLength ?? 0;
}
post({ op: "host-ready" });
</script>`;
}

class Host {
  constructor(glueUrl, generation, onFailure, wasm) {
    this.generation = generation;
    this.onFailure = onFailure;
    this.pending = new Map();
    this.nextId = 0;
    this.dead = false;
    this.retiring = false;
    this.loaded = new Promise((resolve, reject) => {
      this.loadedResolve = resolve;
      this.loadedReject = reject;
    });
    // A rejection seen only through fail() must not be reported unhandled.
    this.loaded.catch(() => {});
    this.loadTimer = setTimeout(
      () => this.fail("prover host did not load"),
      LOAD_TIMEOUT_MS,
    );
    this.frame = document.createElement("iframe");
    this.frame.hidden = true;
    this.frame.setAttribute("aria-hidden", "true");
    this.frame.title = `Mina block prover ${generation}`;
    this.frame.dataset.minaBlockProver = String(generation);
    this.onMessage = event => this.message(event);
    addEventListener("message", this.onMessage);
    // The document's own load follows its "host-ready"; any later load is a
    // navigation away from the prover.
    let loadsAfterReady = 0;
    this.frame.addEventListener("load", () => {
      if (this.readySeen && ++loadsAfterReady > 1)
        this.fail("prover iframe navigated");
    });
    this.observer = new MutationObserver(() => {
      if (!this.frame.isConnected) this.fail("prover iframe removed");
    });
    const parentNode = document.body ?? document.documentElement;
    this.observer.observe(parentNode, { childList: true });
    if (parentNode !== document.documentElement)
      this.observer.observe(document.documentElement, { childList: true });
    this.frame.srcdoc = realmDocument(glueUrl, generation, location.origin, wasm);
    parentNode.append(this.frame);
    this.realm = this.frame.contentWindow;
  }

  message(event) {
    if (event.source !== this.realm || event.source === null) return;
    const data = event.data ?? {};
    if (data.op === "host-ready") {
      clearTimeout(this.loadTimer);
      this.readySeen = true;
      this.loadedResolve();
      return;
    }
    if (data.op === "host-failed") {
      this.fail(data.reason ?? "prover host failed");
      return;
    }
    const item = this.pending.get(data.id);
    if (!item) return;
    clearTimeout(item.timer);
    this.pending.delete(data.id);
    if (data.error !== undefined) {
      const error = new Error(data.error);
      error.fatal = Boolean(data.fatal);
      item.reject(error);
      return;
    }
    if (
      data.buffer !== undefined &&
      (!(data.buffer instanceof ArrayBuffer) ||
        data.buffer.byteLength > MAX_RESPONSE_BYTES)
    ) {
      item.reject(fatalError("block prover response exceeds the size limit"));
      this.fail("block prover response exceeds the size limit");
      return;
    }
    item.resolve(data);
  }

  request(op, fields, transfer, timeoutMs, timeoutReason) {
    if (this.dead)
      return Promise.reject(fatalError(`prover host ${this.generation} is gone`));
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(
        () => this.fail(timeoutReason ?? `${op} timed out`),
        timeoutMs,
      );
      this.pending.set(id, { resolve, reject, timer });
      try {
        this.realm.postMessage({ id, op, ...fields }, location.origin, transfer);
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(fatalError(`prover host ${this.generation}: ${error}`));
      }
    });
  }

  async start(network, threads, key, block, work) {
    await this.loaded;
    const started = performance.now();
    const setup = await this.request(
      "setup",
      { network, threads, block, work },
      [],
      SETUP_TIMEOUT_MS,
      "prover host setup timed out",
    );
    if (block) {
      const keyBuffer = key?.buffer;
      if (!(keyBuffer instanceof ArrayBuffer))
        throw fatalError("a block prover needs the producer key message");
      if (keyBuffer.byteLength > MAX_KEY_BYTES)
        throw fatalError("producer key message is too large");
      await this.request(
        "install-key",
        { key: keyBuffer },
        [keyBuffer],
        KEY_TIMEOUT_MS,
        "producer key installation timed out",
      );
    } else {
      await this.request("work-ready", {}, [], KEY_TIMEOUT_MS, "prover host did not answer");
    }
    const report = JSON.parse(setup.report);
    report.generation = this.generation;
    report.setup_round_trip_ms = performance.now() - started;
    return JSON.stringify(report);
  }

  stopWorkers() {
    try {
      return this.realm?.stopBlockProverWorkers?.() ?? 0;
    } catch {
      return 0;
    }
  }

  fail(reason) {
    if (this.dead) return;
    this.dead = true;
    clearTimeout(this.loadTimer);
    this.loadedReject(fatalError(reason));
    for (const item of this.pending.values()) {
      clearTimeout(item.timer);
      item.reject(fatalError(reason));
    }
    this.pending.clear();
    // Stop the pool while its owner may still be reachable.
    this.stopWorkers();
    observe({ type: "host-failed", generation: this.generation, reason });
    if (!this.retiring) {
      try {
        this.onFailure(this.generation, reason);
      } catch (error) {
        console.error("block prover failure callback failed", error);
      }
    }
  }

  async retire(reason) {
    this.retiring = true;
    const workers = this.stopWorkers();
    this.fail(reason);
    await new Promise(resolve => setTimeout(resolve, RETIRE_GRACE_MS));
    this.observer.disconnect();
    removeEventListener("message", this.onMessage);
    this.frame.remove();
    // Drop the last reference to the old realm, and with it its memory.
    this.realm = null;
    hosts.delete(this.generation);
    observe({ type: "host-retired", generation: this.generation, reason, workers });
    return workers;
  }
}

// `key` is the encoded producer key for a host that proves blocks (`block`),
// null for one that proves only SNARK work (`work`).
export function startBlockProverHost(
  glueUrl,
  generation,
  network,
  threads,
  key,
  block,
  work,
  onFailure,
) {
  try {
    if (!globalThis.crossOriginIsolated)
      throw fatalError("the block prover needs a cross-origin isolated page");
    const host = new Host(glueUrl, generation, onFailure, pinnedWasm());
    hosts.set(generation, host);
    globalThis.__minaBlockProverHosts = hosts;
    observe({ type: "host-started", generation, threads, block, work });
    return host.start(network, threads, key, block, work).then(
      report => {
        observe({ type: "host-ready", generation, report });
        return report;
      },
      error => {
        host.fail(String(error?.message ?? error));
        throw fatalError(String(error?.message ?? error));
      },
    );
  } catch (error) {
    return Promise.reject(
      error.fatal ? error : fatalError(String(error?.message ?? error)),
    );
  }
}

export function proveWithBlockProverHost(
  generation,
  requestId,
  deadlineMs,
  isGenesis,
  request,
) {
  const host = hosts.get(generation);
  if (!host || host.dead)
    return Promise.reject(fatalError(`prover host ${generation} is gone`));
  if (request.byteLength > MAX_REQUEST_BYTES)
    return Promise.reject(new Error("block prover request exceeds the size limit"));
  const started = performance.now();
  observe({ type: "prove-start", generation, requestId, deadlineMs, isGenesis, request: request.slice() });
  const buffer = request.buffer;
  return host
    .request(
      "prove",
      { buffer },
      [buffer],
      Math.max(0, deadlineMs - Date.now()),
      "block proof missed its deadline",
    )
    .then(data => {
      const response = new Uint8Array(data.buffer);
      observe({
        type: "prove-end",
        generation,
        requestId,
        ms: performance.now() - started,
        linearBytes: data.linearBytes,
        response: response.slice(),
      });
      return response;
    });
}

// P5.3: one SNARK work bundle. No slot deadline: `timeoutMs` bounds a hung
// proof, which fails the host.
export function proveWorkWithBlockProverHost(
  generation,
  requestId,
  timeoutMs,
  request,
) {
  const host = hosts.get(generation);
  if (!host || host.dead)
    return Promise.reject(fatalError(`prover host ${generation} is gone`));
  if (request.byteLength > MAX_REQUEST_BYTES)
    return Promise.reject(new Error("SNARK work request exceeds the size limit"));
  const started = performance.now();
  observe({ type: "work-start", generation, requestId, request: request.slice() });
  const buffer = request.buffer;
  return host
    .request(
      "prove-work",
      { buffer },
      [buffer],
      timeoutMs,
      "SNARK work proof timed out",
    )
    .then(data => {
      const response = new Uint8Array(data.buffer);
      observe({
        type: "work-end",
        generation,
        requestId,
        ms: performance.now() - started,
        linearBytes: data.linearBytes,
        response: response.slice(),
      });
      return response;
    });
}

export function retireBlockProverHost(generation, reason) {
  const host = hosts.get(generation);
  if (!host) return Promise.resolve(0);
  return host.retire(reason);
}
