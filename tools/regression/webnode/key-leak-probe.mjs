// Browser/worker instrumentation for the devnet throwaway-key gate. Reports
// contain channel names and counts only; probe configuration stays in memory.
export function installKeyProbe() {
  if (globalThis.__probeInstalled) return;
  globalThis.__probeInstalled = true;
  const hits = (globalThis.__probeHits = {});
  const seen = (globalThis.__probeSeen = {});
  const canary = (globalThis.__probeCanary = {});
  const pending = new Set();
  const tails = new Map();
  const objectIds = new WeakMap();
  let serial = 0;
  const id = object => {
    if (!object || (typeof object !== "object" && typeof object !== "function"))
      return "";
    if (!objectIds.has(object)) objectIds.set(object, ++serial);
    return objectIds.get(object);
  };
  const report = (kind, channel) => {
    const counter = { hits, seen, canary }[kind];
    counter[channel] = (counter[channel] || 0) + 1;
    // CDP worker binding; only counts, never inspected data.
    globalThis.__keyProbeReport?.(JSON.stringify({ kind, channel }));
  };
  const fail = () => report("seen", "probe-error");
  const defer = promise => {
    report("seen", "async-start");
    const task = promise.catch(fail).finally(() => {
      pending.delete(task);
      report("seen", "async-done");
    });
    pending.add(task);
  };
  globalThis.__probeDrain = async () => {
    while (pending.size) await Promise.all([...pending]);
  };
  // A shared probe configuration avoids evaluating code in parked Rust
  // workers. The harness updates it in the document; boundary wrappers read
  // the current version synchronously, even during a long WASM call.
  let shared = null;
  let cachedVersion = -1;
  let cachedConfig = null;
  if (globalThis.document && globalThis.SharedArrayBuffer) {
    shared = globalThis.top?.__keyProbeBuffer ?? new SharedArrayBuffer(65544);
    globalThis.__keyProbeBuffer = shared;
  }
  globalThis.__probeSetConfig = p => {
    globalThis.__minawebProbe = p;
    if (!shared) return;
    const payload = new TextEncoder().encode(JSON.stringify(p));
    if (payload.length > shared.byteLength - 8)
      throw new Error("probe configuration exceeds capacity");
    const header = new Int32Array(shared, 0, 2);
    Atomics.add(header, 0, 1);
    new Uint8Array(shared, 8).set(payload);
    Atomics.store(header, 1, payload.length);
    Atomics.add(header, 0, 1);
  };
  if (globalThis.document && globalThis === globalThis.top)
    globalThis.__probeSetConfig(
      globalThis.__minawebProbe ?? { strings: [], bytes: [], bytesRev: [] },
    );
  const config = () => {
    if (shared) {
      const header = new Int32Array(shared, 0, 2);
      const version = Atomics.load(header, 0);
      if (!(version & 1) && version !== cachedVersion) {
        const length = Atomics.load(header, 1);
        const copy = new Uint8Array(shared, 8, length).slice();
        if (version === Atomics.load(header, 0) && length) {
          cachedConfig = JSON.parse(new TextDecoder().decode(copy));
          cachedVersion = version;
        }
      }
      return cachedConfig;
    }
    return globalThis.__minawebProbe;
  };
  globalThis.__probeKeyCount = () => config()?.strings.length ?? 0;
  // This is a harness-only setup message, delivered before the application's
  // first worker message and hidden from its listeners. Use the native method
  // so the prover's subclass does not mistake instrumentation for a proof job.
  const NativeWorker = globalThis.Worker;
  const awaitingRegistry = new Set();
  let configureChildren = () => {};
  if (NativeWorker) {
    // Capture before wrapping postMessage below: registry delivery is private
    // harness traffic, not an application leak or a prover job.
    const post = NativeWorker.prototype.postMessage;
    const configure = child => post.call(child, { __keyProbeBuffer: shared });
    globalThis.Worker = class extends NativeWorker {
      constructor(...args) {
        super(...args);
        if (shared) configure(this);
        else awaitingRegistry.add(this);
      }
      terminate() {
        awaitingRegistry.delete(this);
        return super.terminate();
      }
    };
    // A worker's initial script can create descendants before the setup
    // message arrives. Flush those children recursively when it does arrive.
    configureChildren = () => {
      for (const child of awaitingRegistry) {
        try {
          configure(child);
        } catch {
          fail();
        }
      }
      awaitingRegistry.clear();
    };
  }
  if (!globalThis.document)
    globalThis.addEventListener?.(
      "message",
      event => {
        const buffer = event.data?.__keyProbeBuffer;
        if (
          globalThis.SharedArrayBuffer &&
          buffer instanceof SharedArrayBuffer &&
          buffer.byteLength === 65544
        ) {
          shared = buffer;
          cachedVersion = -1;
          event.stopImmediatePropagation();
          configureChildren();
          report("seen", "worker-configured");
        }
      },
      true,
    );
  const bytes = value => {
    if (
      value instanceof ArrayBuffer ||
      (globalThis.SharedArrayBuffer && value instanceof SharedArrayBuffer)
    )
      return new Uint8Array(value);
    if (ArrayBuffer.isView(value))
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    if (
      Array.isArray(value) &&
      value.length > 0 &&
      value.every(n => Number.isInteger(n) && n >= 0 && n <= 255)
    )
      return new Uint8Array(value);
    return null;
  };
  const contains = (hay, needle) => {
    outer: for (
      let i = hay.indexOf(needle[0]);
      i >= 0 && i + needle.length <= hay.length;
      i = hay.indexOf(needle[0], i + 1)
    ) {
      for (let j = 1; j < needle.length; j++)
        if (hay[i + j] !== needle[j]) continue outer;
      return true;
    }
    return false;
  };
  let lastConfig, needles, controls;
  const encoder = new TextEncoder();
  function patterns(p) {
    if (lastConfig === p) return;
    lastConfig = p;
    const variants = value => {
      const b =
        typeof value === "string"
          ? encoder.encode(value)
          : new Uint8Array(value);
      const text = Array.from(b, n => String.fromCharCode(n)).join("");
      const hex = Array.from(b, n => n.toString(16).padStart(2, "0")).join("");
      const b64 = btoa(text);
      const out = [
        b,
        encoder.encode(hex),
        encoder.encode(hex.toUpperCase()),
        encoder.encode(b64),
        encoder.encode(
          b64.replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, ""),
        ),
      ];
      if (typeof value === "string") {
        out.push(
          new Uint8Array(Array.from(value, c => [c.charCodeAt(0), 0]).flat()),
        );
        out.push(
          encoder.encode(
            Array.from(b, n => `%${n.toString(16).padStart(2, "0")}`).join(""),
          ),
        );
      }
      return out;
    };
    needles = [...p.strings, ...p.bytes, ...p.bytesRev].flatMap(variants);
    controls = p.canary ? variants(p.canary) : [];
  }
  function scan(channel, value, sink, overlap = true) {
    const p = config();
    if (!p) return;
    patterns(p);
    const b = typeof value === "string" ? encoder.encode(value) : bytes(value);
    if (!b) return;
    // Retain enough overlap to catch encoded values split across writes.
    const key = `${channel}:${id(sink)}`;
    const tail = (overlap && tails.get(key)) || new Uint8Array();
    const joined = new Uint8Array(tail.length + b.length);
    joined.set(tail);
    joined.set(b, tail.length);
    if (needles.some(n => contains(joined, n))) report("hits", channel);
    if (controls.some(n => contains(joined, n))) report("canary", channel);
    if (overlap) tails.set(key, joined.slice(-1024));
  }
  function check(channel, value, sink, visited = new WeakSet()) {
    try {
      report("seen", channel);
      if (value == null) return;
      if (typeof value === "string" || bytes(value)) {
        scan(channel, value, sink);
        return;
      }
      if (typeof value !== "object" || visited.has(value)) return;
      visited.add(value);
      if (globalThis.Blob && value instanceof Blob) {
        defer(value.arrayBuffer().then(b => check(channel, b, sink)));
      } else if (globalThis.ReadableStream && value instanceof ReadableStream) {
        // Callers tee streams before handing a branch here.
        defer(
          (async () => {
            const reader = value.getReader();
            try {
              for (;;) {
                const { done, value: chunk } = await reader.read();
                if (done) break;
                check(channel, chunk, sink);
              }
            } finally {
              reader.releaseLock();
            }
          })(),
        );
      } else if (
        (globalThis.Headers && value instanceof Headers) ||
        (globalThis.FormData && value instanceof FormData)
      ) {
        for (const [k, v] of value.entries()) {
          check(channel, k, sink);
          check(channel, v, sink, visited);
        }
      } else if (
        globalThis.URLSearchParams &&
        value instanceof URLSearchParams
      ) {
        check(channel, String(value), sink);
      } else if (value instanceof Map) {
        for (const [k, v] of value) {
          check(channel, k, sink, visited);
          check(channel, v, sink, visited);
        }
      } else if (value instanceof Set) {
        for (const v of value) check(channel, v, sink, visited);
      } else {
        // WebAssembly.Memory is shared by the node's threads by design. It
        // has no enumerable data; never copy the live multi-GB WASM heap.
        for (const k of Object.keys(value)) {
          if (!Array.isArray(value)) scan(channel, k, sink, false);
          check(channel, value[k], sink, visited);
        }
      }
    } catch {
      fail();
    }
  }
  const wrap = (proto, name, channel) => {
    if (!proto?.[name]) return;
    const original = proto[name];
    proto[name] = function (...args) {
      check(channel, args, this);
      return original.apply(this, args);
    };
  };
  const f = globalThis.fetch;
  const checkHeaders = (headers, sink) => {
    if (headers === undefined) return;
    try {
      // Normalize all HeadersInit forms, including cross-realm Headers.
      check("fetch-headers", [...new Headers(headers)], sink);
    } catch {
      fail();
    }
  };
  if (f)
    globalThis.fetch = function (input, init) {
      check(
        "fetch-url",
        typeof input === "string" ? input : (input.url ?? String(input)),
      );
      checkHeaders(init?.headers, init);
      if (init?.body instanceof ReadableStream) {
        const [send, inspect] = init.body.tee();
        init = { ...init, body: send };
        check("fetch-body", inspect, inspect);
      } else if (init?.body) check("fetch-body", init.body, init);
      if (globalThis.Request && input instanceof Request) {
        checkHeaders(input.headers, input);
        if (input.body) check("fetch-body", input.clone().body, input);
      }
      return f.call(this, input, init);
    };
  wrap(globalThis.XMLHttpRequest?.prototype, "open", "xhr-url");
  wrap(globalThis.XMLHttpRequest?.prototype, "send", "xhr-body");
  wrap(globalThis.WebSocket?.prototype, "send", "websocket");
  wrap(globalThis.RTCDataChannel?.prototype, "send", "webrtc-data");
  wrap(
    globalThis.RTCPeerConnection?.prototype,
    "setLocalDescription",
    "webrtc-sdp",
  );
  wrap(globalThis.navigator, "sendBeacon", "beacon");
  wrap(globalThis.history, "pushState", "history-url");
  wrap(globalThis.history, "replaceState", "history-url");
  wrap(globalThis.Storage?.prototype, "setItem", "storage-write");
  wrap(globalThis.IDBObjectStore?.prototype, "put", "indexeddb-write");
  wrap(globalThis.IDBObjectStore?.prototype, "add", "indexeddb-write");
  // Chromium exposes some OPFS interfaces lazily in workers. Instrument
  // returned handles as well as document APIs, without relying on constructors
  // being present while the worker is paused before its first instruction.
  const patched = new WeakSet();
  function fileWriter(writer, channel) {
    if (patched.has(writer)) return writer;
    patched.add(writer);
    const write = writer.write;
    writer.write = function (data) {
      check(channel, data?.type === "write" ? data.data : data, this);
      return write.apply(this, arguments);
    };
    if (writer.getWriter) {
      const get = writer.getWriter;
      writer.getWriter = function () {
        return fileWriter(get.apply(this, arguments), channel);
      };
    }
    return writer;
  }
  function fileHandle(handle) {
    if (patched.has(handle)) return handle;
    patched.add(handle);
    for (const [name, channel] of [
      ["createWritable", "opfs-write"],
      ["createSyncAccessHandle", "opfs-sync-write"],
    ]) {
      if (!handle[name]) continue;
      const open = handle[name];
      handle[name] = function () {
        return open
          .apply(this, arguments)
          .then(writer => fileWriter(writer, channel));
      };
    }
    return handle;
  }
  function directory(dir) {
    if (patched.has(dir)) return dir;
    patched.add(dir);
    for (const [name, instrument] of [
      ["getFileHandle", fileHandle],
      ["getDirectoryHandle", directory],
    ]) {
      const get = dir[name];
      dir[name] = function (...args) {
        check("opfs-name", args, this);
        return get.apply(this, args).then(instrument);
      };
    }
    return dir;
  }
  const storage = globalThis.navigator?.storage;
  if (storage?.getDirectory) {
    const get = storage.getDirectory;
    storage.getDirectory = function () {
      return get.apply(this, arguments).then(directory);
    };
  }
  wrap(globalThis.Worker?.prototype, "postMessage", "local-worker");
  wrap(globalThis.MessagePort?.prototype, "postMessage", "local-port");
  wrap(
    globalThis.BroadcastChannel?.prototype,
    "postMessage",
    "local-broadcast",
  );
  if (!globalThis.document)
    wrap(globalThis, "postMessage", "worker-postMessage");
  globalThis.addEventListener?.(
    "message",
    event => {
      // Only the actual same-origin prover iframe receiving the install-key
      // operation from its parent may receive secret bytes. Other messages
      // (including broadcasts and worker messages) are findings.
      const d = event.data;
      const intended =
        globalThis.__MINA_BLOCK_PROVER_REALM__ === true &&
        globalThis !== globalThis.top &&
        globalThis.frameElement?.hasAttribute("data-mina-block-prover") &&
        event.source === globalThis.parent &&
        event.origin === globalThis.parent.location.origin &&
        d?.op === "install-key" &&
        Number.isInteger(d.id) &&
        d.key instanceof ArrayBuffer &&
        d.key.byteLength <= 1024 &&
        Object.keys(d).sort().join(",") === "id,key,op";
      check(intended ? "allowed-prover-key" : "local-window", d, event.source);
    },
    true,
  );
  // Console in workers is not consistently forwarded by Page events.
  for (const name of ["log", "info", "warn", "error", "debug"])
    wrap(globalThis.console, name, "console");
}

export const PROBE = `(${installKeyProbe.toString()})();`;

export function probePassed({
  hits = {},
  inspected = {},
  errors = [],
  workers,
  configuredWorkers,
}) {
  return (
    errors.length === 0 &&
    (workers === undefined || workers === configuredWorkers) &&
    !(inspected["probe-error"] > 0) &&
    (inspected["async-start"] ?? 0) === (inspected["async-done"] ?? 0) &&
    Object.entries(hits).every(
      ([channel, count]) => channel === "allowed-prover-key" || count === 0,
    )
  );
}

// Pause every dedicated/shared worker before its first instruction, including
// workers created by the prover iframe or by another worker. The runtime binding
// preserves findings even if a worker terminates before the next collection.
export async function watchWorkers(page, getConfig, counters) {
  const root = await page.createCDPSession();
  const sessions = new Set();
  const pending = new Set();
  counters.workers = 0;
  counters.configuredWorkers = 0;
  counters.errors = [];
  const options = {
    autoAttach: true,
    waitForDebuggerOnStart: true,
    flatten: true,
    filter: [
      { type: "worker", exclude: false },
      { type: "shared_worker", exclude: false },
      { type: "iframe", exclude: false },
    ],
  };
  const evaluate = async (session, expression) => {
    const r = await session.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
    });
    if (r.exceptionDetails) throw new Error("worker probe evaluation failed");
  };
  const config = () =>
    `globalThis.__minawebProbe = ${JSON.stringify(getConfig())};`;
  async function attach(parent, event) {
    const session = parent.connection().session(event.sessionId);
    if (!session) throw new Error("worker CDP session unavailable");
    listen(session);
    await session.send("Target.setAutoAttach", options);
    if (event.targetInfo.type.includes("worker")) {
      sessions.add(session);
      let configured = false;

      session.on("Runtime.bindingCalled", ({ name, payload }) => {
        if (name !== "__keyProbeReport") return;
        try {
          const { kind, channel } = JSON.parse(payload);
          const bucket = counters[kind === "seen" ? "inspected" : kind];
          if (!bucket || typeof channel !== "string") throw new Error();
          bucket[channel] = (bucket[channel] || 0) + 1;
          if (
            kind === "seen" &&
            channel === "worker-configured" &&
            !configured
          ) {
            configured = true;
            counters.configuredWorkers++;
          }
        } catch {
          counters.errors.push("invalid worker probe report");
        }
      });
      await session.send("Runtime.enable");
      await session.send("Runtime.addBinding", { name: "__keyProbeReport" });
      const initial = config();
      await evaluate(session, initial + PROBE);
      counters.workers++;
    }
    await session.send("Runtime.runIfWaitingForDebugger");
  }
  function listen(session) {
    session.on("Target.attachedToTarget", event => {
      const task = attach(session, event)
        .catch(() => {
          counters.errors.push("worker probe installation failed");
        })
        .finally(() => pending.delete(task));
      pending.add(task);
    });
  }
  listen(root);
  await root.send("Target.setAutoAttach", options);
  return {
    async update() {
      // Configuration is shared; no evaluation in busy/parked workers.
      await Promise.all([...pending]);
    },
    async drain() {
      await Promise.all([...pending]);
      // Bindings report immediately. An unfinished asynchronous scan is a
      // coverage failure via async-start/async-done, even after worker exit.
      for (const s of sessions) if (s.detached) sessions.delete(s);
    },
  };
}

// Positive controls contain no key material and leave no records behind.
// Drive actual APIs, including nested Blobs, streamed Request bodies and OPFS.
export async function exerciseProbe(canary) {
  await fetch(`/probe-control?k=${canary}`).catch(() => {});
  await fetch("/probe-control", {
    headers: new Headers({ "X-Probe-Control": canary }),
  }).catch(() => {});
  const form = new FormData();
  form.append("probe", new Blob([canary]));
  await fetch("/probe-control", { method: "POST", body: form }).catch(() => {});
  await fetch(
    new Request(new URL("/probe-control", location.origin), {
      method: "POST",
      body: new Blob([canary]),
    }),
  ).catch(() => {});
  const encoded = new TextEncoder().encode(canary);
  const stream = new ReadableStream({
    start(c) {
      c.enqueue(encoded.slice(0, 9));
      c.enqueue(encoded.slice(9));
      c.close();
    },
  });
  await fetch("/probe-control", {
    method: "POST",
    body: stream,
    duplex: "half",
  }).catch(() => {});
  if (globalThis.localStorage) {
    localStorage.setItem("probe-control", canary);
    localStorage.removeItem("probe-control");
  }
  const db = await new Promise((resolve, reject) => {
    const req = indexedDB.open("probe-control", 1);
    req.onupgradeneeded = () => req.result.createObjectStore("probe");
    req.onerror = () => reject(new Error("probe IndexedDB control failed"));
    req.onsuccess = () => resolve(req.result);
  });
  await new Promise((resolve, reject) => {
    const tx = db.transaction("probe", "readwrite");
    tx.objectStore("probe").put({ blob: new Blob([canary]) }, "probe");
    tx.objectStore("probe").delete("probe");
    tx.oncomplete = resolve;
    tx.onerror = () => reject(new Error("probe IndexedDB write failed"));
  });
  db.close();
  indexedDB.deleteDatabase("probe-control");
  const dir = await navigator.storage.getDirectory();
  const handle = await dir.getFileHandle("probe-control", { create: true });
  if (globalThis.document) {
    const writer = await handle.createWritable();
    await writer.write(encoded.slice(0, 9));
    await writer.write({ type: "write", position: 9, data: encoded.slice(9) });
    await writer.close();
  } else {
    const writer = await handle.createSyncAccessHandle();
    writer.write(encoded.slice(0, 9));
    writer.write(encoded.slice(9), { at: 9 });
    writer.close();
  }
  await dir.removeEntry("probe-control");
  const port = new MessageChannel();
  port.port1.postMessage(btoa(canary));
  port.port1.close();
  port.port2.close();
  const broadcast = new BroadcastChannel("probe-control");
  broadcast.postMessage(canary);
  broadcast.close();
  if (!globalThis.document) postMessage(canary);
  console.log("probe control", canary);
  await globalThis.__probeDrain();
}
