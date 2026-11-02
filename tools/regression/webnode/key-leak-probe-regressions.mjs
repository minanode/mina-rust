// Run in a remote browser slot. Synthetic values only; evidence holds counts.
// LEAK_CASE=storage is a deliberately failing gate (expected process exit 1).
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { PROBE, probePassed, watchWorkers } from "./key-leak-probe.mjs";
import { findPuppeteer } from "./lib.mjs";

const fake = `synthetic-probe-${randomBytes(20).toString("hex")}`;
const received = {};
const server = createServer((req, res) => {
  if (req.headers["x-probe-secret"] === fake)
    received[req.url] = (received[req.url] ?? 0) + 1;
  req.resume();
  res.writeHead(200, {
    "Content-Type": "text/html",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
  });
  res.end("<!doctype html><title>Probe regressions</title>");
});
await new Promise(resolve =>
  server.listen(Number(process.env.STATIC_PORT), "127.0.0.1", resolve),
);
const { puppeteer } = findPuppeteer();
const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  args: ["--no-sandbox"],
});
const result = {};
const origin = `http://127.0.0.1:${process.env.STATIC_PORT}`;
async function open(strings) {
  const config = { strings, bytes: [], bytesRev: [] };
  const page = await browser.newPage();
  const workers = { hits: {}, inspected: {}, canary: {} };
  const watcher = await watchWorkers(page, () => config, workers);
  await page.evaluateOnNewDocument(p => {
    globalThis.__minawebProbe = p;
  }, config);
  await page.evaluateOnNewDocument(PROBE);
  await page.goto(origin);
  return { page, workers, watcher };
}
// Each shape has its own counter delta: another detected header cannot mask it.
async function headers(key, base, realm) {
  const results = {};
  for (const form of ["Headers", "pairs", "object", "Request"]) {
    const before = globalThis.__probeHits["fetch-headers"] ?? 0;
    const url = `${base}/headers/${realm}/${form}`;
    const pairs = [["X-Probe-Secret", key]];
    if (form === "Request") {
      const request = new Request(url);
      request.headers.set("X-Probe-Secret", key);
      await fetch(request);
    } else {
      const value =
        form === "Headers"
          ? new Headers(pairs)
          : form === "pairs"
            ? pairs
            : Object.fromEntries(pairs);
      await fetch(url, { headers: value });
    }
    results[form] = (globalThis.__probeHits["fetch-headers"] ?? 0) - before;
  }
  return results;
}
try {
  if (process.env.LEAK_CASE === "storage") {
    const { page } = await open([fake]);
    result.storage = await page.evaluate(key => {
      localStorage.setItem("synthetic-probe", key);
      localStorage.removeItem("synthetic-probe");
      return {
        hits: globalThis.__probeHits,
        inspected: globalThis.__probeSeen,
      };
    }, fake);
    process.exitCode = probePassed(result.storage) ? 0 : 1;
  } else {
    const h = await open([fake]);
    result.documentHeaders = await h.page.evaluate(
      headers,
      fake,
      origin,
      "document",
    );
    result.workerHeaders = await h.page.evaluate(
      async (fn, key, base) => {
        const code = `(${fn})(${JSON.stringify(key)}, ${JSON.stringify(base)}, "worker").then(postMessage);`;
        const url = URL.createObjectURL(
          new Blob([code], { type: "text/javascript" }),
        );
        const worker = new Worker(url);
        try {
          return await new Promise((resolve, reject) => {
            const timer = setTimeout(
              () => reject(new Error("header worker timeout")),
              15000,
            );
            worker.onmessage = e => {
              clearTimeout(timer);
              resolve(e.data);
            };
            worker.onerror = () => reject(new Error("header worker failed"));
          });
        } finally {
          worker.terminate();
          URL.revokeObjectURL(url);
        }
      },
      headers.toString(),
      fake,
      origin,
    );
    await h.watcher.drain();
    result.headerWorkers = h.workers;
    await h.page.close();

    const n = await open([]);
    result.nested = await n.page.evaluate(async key => {
      // Each script creates its child before it can handle the asynchronous
      // registry message. The third generation exposes recursive propagation.
      function code(level, key) {
        const channel = new BroadcastChannel("nested-probe");
        const children = [];
        const spawn = next => {
          const url = URL.createObjectURL(
            new Blob(
              [`(${code.toString()})(${next}, ${JSON.stringify(key)});`],
              { type: "text/javascript" },
            ),
          );
          children.push(new Worker(url));
          URL.revokeObjectURL(url);
        };
        if (level < 2) spawn(level + 1);
        channel.onmessage = e => {
          if (e.data === "spawn" && level === 0) spawn(3);
          if (e.data === "leak") {
            const before = globalThis.__probeHits["local-broadcast"] ?? 0;
            channel.postMessage(key);
            channel.postMessage({
              level,
              keyCount: globalThis.__probeKeyCount(),
              hits: (globalThis.__probeHits["local-broadcast"] ?? 0) - before,
            });
          }
          if (e.data === "stop") {
            children.forEach(w => w.terminate());
            channel.close();
            close();
          }
        };
        channel.postMessage({ ready: level });
      }
      const channel = new BroadcastChannel("nested-probe");
      const ready = new Set();
      let replies = [];
      channel.onmessage = e => {
        if (Number.isInteger(e.data?.ready)) ready.add(e.data.ready);
        if (Number.isInteger(e.data?.level)) replies.push(e.data);
      };
      const until = async check => {
        const end = Date.now() + 15000;
        while (!check()) {
          if (Date.now() > end) throw new Error("nested worker timeout");
          await new Promise(resolve => setTimeout(resolve, 20));
        }
      };
      const url = URL.createObjectURL(
        new Blob([`(${code.toString()})(0, ${JSON.stringify(key)});`], {
          type: "text/javascript",
        }),
      );
      const outer = new Worker(url);
      try {
        await until(() => ready.size === 3);
        globalThis.__probeSetConfig({
          strings: [key],
          bytes: [],
          bytesRev: [],
        });
        channel.postMessage("leak");
        await until(() => replies.length === 3);
        const existing = replies;
        replies = [];
        channel.postMessage("spawn");
        await until(() => ready.size === 4);
        channel.postMessage("leak");
        await until(() => replies.length === 4);
        return { existing, future: replies };
      } finally {
        channel.postMessage("stop");
        await new Promise(resolve => setTimeout(resolve, 50));
        outer.terminate();
        channel.close();
        URL.revokeObjectURL(url);
      }
    }, fake);
    await n.watcher.drain();
    result.nestedWorkers = n.workers;
    result.missingRegistryFails = !probePassed({
      workers: 2,
      configuredWorkers: 1,
    });
    result.received = received;
    result.ok =
      Object.values(result.documentHeaders).every(n => n > 0) &&
      Object.values(result.workerHeaders).every(n => n > 0) &&
      Object.keys(received).length === 8 &&
      [...result.nested.existing, ...result.nested.future].every(
        r => r.keyCount === 1 && r.hits > 0,
      ) &&
      result.missingRegistryFails &&
      !probePassed(n.workers) &&
      !probePassed(h.workers);
    assert.equal(
      result.ok,
      true,
      "nested workers, header forms and missing registry must fail the leak gate",
    );
  }
} finally {
  writeFileSync(
    join(process.env.OUT, "probe-regressions.json"),
    JSON.stringify(result, null, 2),
  );
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
