// Real Chromium positive/negative controls, run in a remote browser slot.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  PROBE,
  exerciseProbe,
  watchWorkers,
  probePassed,
} from "./key-leak-probe.mjs";
import { findPuppeteer } from "./lib.mjs";

const { puppeteer } = findPuppeteer();
const { CHROME, STATIC_PORT, OUT } = process.env;
const server = createServer((req, res) => {
  req.resume();
  res.writeHead(200, {
    "Content-Type": "text/html",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
  });
  res.end("<!doctype html><title>Probe controls</title>");
});
await new Promise(resolve =>
  server.listen(Number(STATIC_PORT), "127.0.0.1", resolve),
);
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: true,
  args: ["--no-sandbox"],
});
try {
  const canary = `probe-control-${randomBytes(20).toString("hex")}`;
  const config = { strings: [canary], bytes: [], bytesRev: [], canary };
  const workers = { hits: {}, inspected: {}, canary: {} };
  const page = await browser.newPage();
  const watcher = await watchWorkers(page, () => config, workers);
  await page.evaluateOnNewDocument(p => {
    globalThis.__minawebProbe = p;
  }, config);
  await page.evaluateOnNewDocument(PROBE);
  await page.goto(`http://127.0.0.1:${STATIC_PORT}`);
  assert.equal(
    await page.evaluate(() => Object.keys(globalThis.__probeHits).length),
    0,
  );
  await page.evaluate(exerciseProbe, canary);
  await page.evaluate(
    async (source, c) => {
      const url = URL.createObjectURL(
        new Blob(
          [
            `(${source})(${JSON.stringify(c)}).then(() => postMessage("done"));`,
          ],
          { type: "text/javascript" },
        ),
      );
      const w = new Worker(url);
      try {
        await new Promise((resolve, reject) => {
          const timer = setTimeout(
            () => reject(new Error("worker control timeout")),
            30000,
          );
          w.onmessage = e => {
            if (e.data === "done") {
              clearTimeout(timer);
              resolve();
            }
          };
          w.onerror = () => reject(new Error("worker control error"));
        });
      } finally {
        w.terminate();
        URL.revokeObjectURL(url);
      }
    },
    exerciseProbe.toString(),
    canary,
  );
  await watcher.drain();
  const frames = await page.evaluate(() => ({
    hits: globalThis.__probeHits,
    inspected: globalThis.__probeSeen,
  }));
  writeFileSync(
    join(OUT, "probe-controls.json"),
    JSON.stringify({ frames, workers }, null, 2),
  );
  for (const k of [
    "fetch-url",
    "fetch-headers",
    "fetch-body",
    "storage-write",
    "indexeddb-write",
    "opfs-write",
    "local-port",
    "local-broadcast",
    "console",
  ])
    assert.ok(frames.hits[k] > 0, `document ${k}`);
  for (const k of [
    "fetch-url",
    "fetch-headers",
    "fetch-body",
    "indexeddb-write",
    "opfs-sync-write",
    "local-port",
    "local-broadcast",
    "worker-postMessage",
    "console",
  ])
    assert.ok(workers.hits[k] > 0, `worker ${k}`);
  assert.equal(frames.inspected["probe-error"] ?? 0, 0);
  assert.deepEqual(workers.errors, []);
  assert.equal(probePassed(frames), false);
  assert.equal(probePassed(workers), false);
  assert.equal(probePassed({}), true);
  assert.equal(probePassed({ hits: { "allowed-prover-key": 1 } }), true);
  for (const channel of [
    "local-worker",
    "local-port",
    "local-window",
    "local-broadcast",
    "worker-postMessage",
    "opfs-write",
    "fetch-body",
  ])
    assert.equal(probePassed({ hits: { [channel]: 1 } }), false, channel);
  assert.equal(probePassed({ inspected: { "probe-error": 1 } }), false);
  assert.equal(probePassed({ errors: ["missing worker"] }), false);
  // Values split across byte chunks and encoded as base64/hex must be found.
  const before = frames.hits["local-port"];
  await page.evaluate(c => {
    const { port1, port2 } = new MessageChannel();
    const b = new TextEncoder().encode(c);
    port1.postMessage(b.slice(0, 11));
    port1.postMessage(b.slice(11));
    port1.postMessage(
      Array.from(b, x => x.toString(16).padStart(2, "0")).join(""),
    );
    port1.postMessage(btoa(c));
    port1.close();
    port2.close();
  }, canary);
  assert.ok(
    (await page.evaluate(() => globalThis.__probeHits["local-port"])) >=
      before + 3,
  );
  // Refresh configuration while a worker is parked in Atomics.wait. No CDP
  // evaluation or worker event-loop turn may be needed to see the new value.
  const late = `late-control-${randomBytes(20).toString("hex")}`;
  const beforeWorker = workers.hits["worker-postMessage"];
  await page.evaluate(
    async (c, late) => {
      const signal = new SharedArrayBuffer(4);
      const code = `onmessage = e => { postMessage("parked"); Atomics.wait(new Int32Array(e.data), 0, 0, 10000); postMessage(${JSON.stringify(late)}); };`;
      const url = URL.createObjectURL(
        new Blob([code], { type: "text/javascript" }),
      );
      const w = new Worker(url);
      await new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("parked worker control timeout")),
          15000,
        );
        w.onmessage = e => {
          if (e.data === "parked") {
            globalThis.__probeSetConfig({
              strings: [c, late],
              bytes: [],
              bytesRev: [],
              canary: c,
            });
            Atomics.store(new Int32Array(signal), 0, 1);
            Atomics.notify(new Int32Array(signal), 0);
          } else if (e.data === late) {
            clearTimeout(timer);
            resolve();
          }
        };
        w.postMessage(signal);
      });
      w.terminate();
      URL.revokeObjectURL(url);
      // A newly installed iframe must inherit, never erase, the key registry.
      const frame = document.createElement("iframe");
      frame.dataset.minaBlockProver = "1";
      frame.srcdoc =
        "<script>globalThis.__MINA_BLOCK_PROVER_REALM__ = true;</script>";
      document.body.append(frame);
      await new Promise(resolve => (frame.onload = resolve));
      if (frame.contentWindow.__probeKeyCount() !== 2)
        throw new Error("iframe lost probe configuration");
      frame.contentWindow.postMessage(
        { id: 1, op: "install-key", key: new TextEncoder().encode(c).buffer },
        location.origin,
      );
      await new Promise(resolve => setTimeout(resolve, 50));
      if (!frame.contentWindow.__probeHits["allowed-prover-key"])
        throw new Error("intended key transfer not recognized");
      frame.remove();
      globalThis.__MINA_BLOCK_PROVER_REALM__ = true;
      postMessage(
        { id: 1, op: "install-key", key: new TextEncoder().encode(c).buffer },
        location.origin,
      );
      await new Promise(resolve => setTimeout(resolve, 50));
      if (!globalThis.__probeHits["local-window"])
        throw new Error("unexpected local transfer was allowed");
    },
    canary,
    late,
  );
  await watcher.drain();
  assert.ok(
    workers.hits["worker-postMessage"] > beforeWorker,
    "late key registration in parked worker",
  );
  writeFileSync(
    join(OUT, "probe-controls.json"),
    JSON.stringify({ ok: true, frames, workers }, null, 2),
  );
  console.log(
    "PASS: worker/document controls; transient storage, IndexedDB, OPFS, streams, encodings, chunks, and failure gate",
  );
} finally {
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
