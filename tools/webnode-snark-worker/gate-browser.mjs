#!/usr/bin/env node
// P5.3 gate: the web node in headless Chrome as the private network's SNARK
// worker (www/gate.mjs), in the run's network namespace beside our relay and
// the OCaml producer. Take the browser lock and enter the namespace outside:
//   tools/regression/lock.sh browser -- \
//     bash tools/local-mesa-net/enter.sh RUN node tools/webnode-snark-worker/gate-browser.mjs
//
// Environment:
//   RUN       the run directory; RUN/public holds genesis.bin and worker.json
//   PKG       the web node's wasm-bindgen output
//   BLOBS     circuit blobs, the parent of 4.0.2devnet
//   OUT       where results go (created)
//   RELAY     the relay's loopback WebRTC multiaddr
//   PORT      the page server's port (not 4200-4205 or 4222-4224)
//   THREADS   prover threads (default 8)
//   TIMEOUT_MS  the most the browser runs (default 150 minutes)
//
// The namespace has no route out; Chrome resolves no hostname, and an
// internal interface lets WebRTC gather a host candidate (P4.12). The runner
// owns only its Chrome and page server and closes both in `finally`. It
// writes OUT/result.json every minute and at the end, and stops when
// OUT/stop appears, on SIGTERM, or at TIMEOUT_MS. It samples the browser's
// memory from /proc every 5 s (summed RSS and PSS of Chrome's processes).
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../..");
const main = path.dirname(
  spawnSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], {
    cwd: repo,
    encoding: "utf8",
  }).stdout.trim(),
);
const require = createRequire(
  path.join(
    process.env.PUPPETEER_DIR ?? path.join(main, "crates/web-verifier/tests/browser"),
    "package.json",
  ),
);
const puppeteer = require("puppeteer-core");
const need = name => {
  const value = process.env[name];
  if (!value) throw new Error(`set ${name}`);
  return value;
};
const run = path.resolve(need("RUN"));
const pkg = path.resolve(need("PKG"));
const blobs = path.resolve(need("BLOBS"));
const out = path.resolve(need("OUT"));
const relay = need("RELAY");
const port = Number(need("PORT"));
if ((port >= 4200 && port <= 4205) || (port >= 4222 && port <= 4224))
  throw new Error(`port ${port} is reserved`);
if (!/^\/ip4\/127\.0\.0\.1\/tcp\/\d+\/webrtc\/http\/p2p\/[A-Za-z0-9]+$/.test(relay))
  throw new Error("RELAY must be a loopback WebRTC address");
const timeoutMs = Number(process.env.TIMEOUT_MS ?? 150 * 60 * 1000);
const chrome =
  process.env.CHROME ??
  path.join(
    os.homedir(),
    ".local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell",
  );
const sha256 = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");

fs.mkdirSync(path.join(out, "artifacts"), { recursive: true });
const log = fs.createWriteStream(path.join(out, "console.log"));
const serverLog = fs.openSync(path.join(out, "server.log"), "w");
const server = spawn(
  "python3",
  [path.join(here, "serve.py"), "--pkg", pkg, "--blobs", blobs, "--run", run, "--port", String(port)],
  { stdio: ["ignore", serverLog, serverLog] },
);
let browser;
let stopping = false;
async function cleanup() {
  if (stopping) return;
  stopping = true;
  try {
    if (browser) await browser.close();
  } finally {
    if (server.exitCode === null && server.signalCode === null) {
      const exited = new Promise(resolve => server.once("exit", resolve));
      server.kill("SIGTERM");
      await exited;
    }
    log.end();
    fs.closeSync(serverLog);
  }
}
let stopRequested = false;
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => (stopRequested = true));

function tree(root) {
  const children = new Map();
  for (const pid of fs.readdirSync("/proc").filter(p => /^\d+$/.test(p))) {
    try {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
      const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
      if (!children.has(ppid)) children.set(ppid, []);
      children.get(ppid).push(Number(pid));
    } catch {}
  }
  const all = [root];
  for (let i = 0; i < all.length; i++) all.push(...(children.get(all[i]) ?? []));
  return all;
}
function browserMemory(root) {
  let rss = 0;
  let pss = 0;
  for (const pid of tree(root)) {
    try {
      const rollup = fs.readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
      const kb = name => Number(new RegExp(`^${name}:\\s+(\\d+) kB`, "m").exec(rollup)?.[1] ?? 0);
      rss += kb("Rss");
      pss += kb("Pss");
    } catch {}
  }
  return { rssKiB: rss, pssKiB: pss };
}

const started = Date.now();
const elapsed = () => ((Date.now() - started) / 1000).toFixed(1);
const samples = [];
let peak = { rssKiB: 0, pssKiB: 0 };
let page;
let fatalError;
async function snapshot(final) {
  const result = (await page?.evaluate(() => window.harness).catch(() => null)) ?? {};
  result.final = final;
  result.pageError = fatalError ? String(fatalError.stack ?? fatalError) : null;
  result.browserMemory = { peak, samples };
  result.machine = {
    cpu: os.cpus()[0].model,
    threads: os.cpus().length,
    chrome: browser ? await browser.version() : null,
    load: os.loadavg(),
  };
  result.artifacts = {
    commit: spawnSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8" }).stdout.trim(),
    dirty: Boolean(spawnSync("git", ["status", "--porcelain"], { cwd: repo, encoding: "utf8" }).stdout.trim()),
    wasmSha256: sha256(path.join(pkg, "mina_node_web_bg.wasm")),
    genesisSha256: sha256(path.join(run, "public/genesis.bin")),
  };
  const file = path.join(out, "result.json");
  fs.writeFileSync(file + ".tmp", JSON.stringify(result, null, 2));
  fs.renameSync(file + ".tmp", file);
  return result;
}
try {
  for (let n = 0; ; n++) {
    if (server.exitCode !== null) throw new Error("server exited; see server.log");
    if (fs.readFileSync(path.join(out, "server.log"), "utf8").includes("ready")) break;
    if (n === 50) throw new Error("server did not start");
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--allow-loopback-in-peer-connection",
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
    ],
    env: {
      ...process.env,
      LD_LIBRARY_PATH:
        process.env.LD_LIBRARY_PATH ??
        path.join(os.homedir(), ".local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu"),
    },
    protocolTimeout: timeoutMs + 60000,
  });
  const chromePid = browser.process().pid;
  fs.writeFileSync(
    path.join(out, "pids.json"),
    JSON.stringify({ runner: process.pid, server: server.pid, chrome: chromePid }) + "\n",
  );
  const sampler = setInterval(() => {
    const sample = { at: Date.now(), ...browserMemory(chromePid), load: os.loadavg()[0] };
    if (sample.rssKiB > peak.rssKiB) peak = { ...peak, rssKiB: sample.rssKiB, rssAt: sample.at };
    if (sample.pssKiB > peak.pssKiB) peak = { ...peak, pssKiB: sample.pssKiB, pssAt: sample.at };
    const last = samples.at(-1);
    if (!last || sample.at - last.at >= 30000) samples.push(sample);
  }, 5000);
  page = await browser.newPage();
  page.on("console", msg => log.write(`${new Date().toISOString()} ${msg.type()}: ${msg.text()}\n`));
  page.on("pageerror", error => {
    log.write(`${new Date().toISOString()} pageerror: ${error.stack}\n`);
    if (/Cursed hack to keep workers alive/.test(String(error?.message))) return;
    fatalError ??= error;
  });
  await page.exposeFunction("saveArtifact", (name, bytes) => {
    if (!/^work-(request|response)-\d+-\d+\.bin$/.test(name))
      throw new Error(`unexpected artifact ${name}`);
    fs.writeFileSync(path.join(out, "artifacts", name), Buffer.from(bytes));
  });
  const query = new URLSearchParams({ relay, threads: process.env.THREADS ?? "8" });
  await page.goto(`http://127.0.0.1:${port}/gate.html?${query}`);
  const deadline = Date.now() + timeoutMs;
  let lastSave = 0;
  while (Date.now() < deadline && !stopRequested && !fs.existsSync(path.join(out, "stop"))) {
    if (fatalError) break;
    if (Date.now() - lastSave >= 60000) {
      const result = await snapshot(false);
      lastSave = Date.now();
      if (result.error) break;
      const proved = (result.events ?? []).filter(e => e.type === "work-end").length;
      console.log(`${elapsed()}s: bundles proved ${proved}, phase ${result.phase}`);
    }
    await new Promise(resolve => setTimeout(resolve, 2000));
  }
  clearInterval(sampler);
  const result = await snapshot(true);
  console.log(`${elapsed()}s: stopped; error=${result.error ?? null}; ${out}/result.json`);
  if (result.error || result.pageError) throw new Error(result.error ?? result.pageError);
} catch (error) {
  fs.writeFileSync(path.join(out, "error.json"), JSON.stringify({ error: String(error.stack ?? error) }, null, 2));
  throw error;
} finally {
  await cleanup();
}
