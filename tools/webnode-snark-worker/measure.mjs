#!/usr/bin/env node
// P5.3: measure the web node's isolated prover with the transaction
// circuits, next to the block circuits and alone, in headless Chrome
// (www/measure.mjs). Take the browser lock outside:
//   tools/regression/lock.sh browser -- node tools/webnode-snark-worker/measure.mjs
//
// Environment:
//   PKG       the web node's wasm-bindgen output (required)
//   OUT       where results go (required, created)
//   BLOBS     circuit blobs, the parent of 4.0.2devnet (required)
//   PORT      the page server's port (required; not 4200-4205 or 4222-4224)
//   THREADS   prover threads (default 8)
//   CONFIGS   comma-separated: work, both, block (default work,both,block)
//   REPEAT    run each configuration's plan this many times (default 1)
//   TIMEOUT_MS (default 40 minutes)
//   CHROME, PUPPETEER_DIR as for tools/webnode-block-prover/run.mjs
//
// Chrome reaches 127.0.0.1 only: a closed proxy, no non-proxied UDP, and
// every hostname unresolvable. The runner owns only its Chrome and page
// server, records their PIDs, and closes both in `finally`. Besides the
// page's own measurements it samples the browser's memory from /proc every
// 2 s: the summed RSS and PSS of Chrome's processes.
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
const pkg = path.resolve(need("PKG"));
const out = path.resolve(need("OUT"));
const blobs = path.resolve(need("BLOBS"));
const port = Number(need("PORT"));
if ((port >= 4200 && port <= 4205) || (port >= 4222 && port <= 4224))
  throw new Error(`port ${port} is reserved`);
const timeoutMs = Number(process.env.TIMEOUT_MS ?? 40 * 60 * 1000);
const chrome =
  process.env.CHROME ??
  path.join(
    os.homedir(),
    ".local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell",
  );
const sha256 = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");

// The host module is a wasm-bindgen snippet: one per build, under a hash.
const snippets = fs
  .readdirSync(path.join(pkg, "snippets"))
  .map(dir => path.join("snippets", dir, "js/block_prover_host.js"))
  .filter(file => fs.existsSync(path.join(pkg, file)));
if (snippets.length !== 1) throw new Error(`expected one host snippet, found ${snippets}`);

fs.mkdirSync(path.join(out, "artifacts"), { recursive: true });
const log = fs.createWriteStream(path.join(out, "console.log"));
const serverLog = fs.openSync(path.join(out, "server.log"), "w");
const server = spawn(
  "python3",
  [path.join(here, "serve.py"), "--pkg", pkg, "--blobs", blobs, "--port", String(port)],
  { stdio: ["ignore", serverLog, serverLog] },
);
fs.writeFileSync(path.join(out, "pids.json"), JSON.stringify({ server: server.pid }) + "\n");
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
for (const signal of ["SIGINT", "SIGTERM"])
  process.on(signal, () => {
    cleanup().finally(() => process.exit(130));
  });

/// The browser's processes: Chrome and its descendants.
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
  const processes = [];
  for (const pid of tree(root)) {
    try {
      const rollup = fs.readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
      const kb = name => Number(new RegExp(`^${name}:\\s+(\\d+) kB`, "m").exec(rollup)?.[1] ?? 0);
      const cmd = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
      const type = /--type=([a-z-]+)/.exec(cmd.join(" "))?.[1] ?? "browser";
      rss += kb("Rss");
      pss += kb("Pss");
      processes.push({ pid, type, rssKiB: kb("Rss"), pssKiB: kb("Pss") });
    } catch {}
  }
  return { rssKiB: rss, pssKiB: pss, processes };
}

const started = Date.now();
const elapsed = () => ((Date.now() - started) / 1000).toFixed(1);
const memory = [];
let peak = { rssKiB: 0, pssKiB: 0 };
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
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
      "--proxy-server=http://127.0.0.1:9",
      "--proxy-bypass-list=127.0.0.1;localhost",
      "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
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
    JSON.stringify({ server: server.pid, chrome: chromePid }) + "\n",
  );
  const sampler = setInterval(() => {
    const sample = browserMemory(chromePid);
    if (sample.rssKiB > peak.rssKiB) peak = { ...peak, rssKiB: sample.rssKiB, rssAt: Date.now() };
    if (sample.pssKiB > peak.pssKiB) peak = { ...peak, pssKiB: sample.pssKiB, pssAt: Date.now(), processes: sample.processes };
    memory.push({ at: Date.now(), rssKiB: sample.rssKiB, pssKiB: sample.pssKiB, load: os.loadavg()[0] });
  }, 2000);
  const loadBefore = os.loadavg();
  const page = await browser.newPage();
  page.on("console", msg => log.write(`${elapsed()}s ${msg.type()}: ${msg.text()}\n`));
  let fatalError;
  page.on("pageerror", error => {
    log.write(`${elapsed()}s pageerror: ${error.stack}\n`);
    if (/Cursed hack to keep workers alive/.test(String(error?.message))) return;
    fatalError ??= error;
  });
  await page.exposeFunction("saveArtifact", (name, bytes) => {
    if (!/^[a-z]+-(request|response)-\d+-(block|work)\.bin$/.test(name))
      throw new Error(`unexpected artifact ${name}`);
    fs.writeFileSync(path.join(out, "artifacts", name), Buffer.from(bytes));
  });
  const cdp = await browser.target().createCDPSession();
  await page.exposeFunction("cdpWorkers", async () => {
    const workers = (await cdp.send("Target.getTargets")).targetInfos.filter(t => t.type === "worker");
    return { total: workers.length };
  });
  const query = new URLSearchParams({
    threads: process.env.THREADS ?? "8",
    network: "devnet",
    configs: process.env.CONFIGS ?? "work,both,block",
    repeat: process.env.REPEAT ?? "1",
    snippet: `/pkg/${snippets[0]}`,
  });
  await page.goto(`http://127.0.0.1:${port}/measure.html?${query}`);
  const deadline = Date.now() + timeoutMs;
  let progress;
  while (Date.now() < deadline) {
    progress = await page.evaluate(() => ({ done: window.harness?.done, error: window.harness?.error }));
    if (progress.done || progress.error || fatalError) break;
    await new Promise(resolve => setTimeout(resolve, 5000));
  }
  clearInterval(sampler);
  const result = await page.evaluate(() => window.harness);
  result.timedOut = !(result.done || result.error);
  result.pageError = fatalError ? String(fatalError.stack ?? fatalError) : null;
  result.browserMemory = { peak, samples: memory };
  result.machine = {
    cpu: os.cpus()[0].model,
    threads: os.cpus().length,
    chrome: await browser.version(),
    loadBefore,
    loadAfter: os.loadavg(),
  };
  result.artifacts = {
    commit: spawnSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8" }).stdout.trim(),
    dirty: Boolean(spawnSync("git", ["status", "--porcelain"], { cwd: repo, encoding: "utf8" }).stdout.trim()),
    wasmSha256: sha256(path.join(pkg, "mina_node_web_bg.wasm")),
  };
  fs.writeFileSync(path.join(out, "result.json"), JSON.stringify(result, null, 2));
  console.log(`${elapsed()}s: done=${result.done} error=${result.error ?? null}; ${out}/result.json`);
  if (result.error || result.pageError) throw new Error(result.error ?? result.pageError);
  if (result.timedOut) throw new Error("timed out");
} catch (error) {
  fs.writeFileSync(path.join(out, "error.json"), JSON.stringify({ error: String(error.stack ?? error) }, null, 2));
  throw error;
} finally {
  await cleanup();
}
