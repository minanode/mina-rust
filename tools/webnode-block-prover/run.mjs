#!/usr/bin/env node
// I34c: run the web node alone on a private Mesa chain in headless Chrome,
// let it prove and produce its own blocks through its isolated prover, and
// verify every proof that crossed the bridge natively. Take the browser
// lock outside: tools/regression/lock.sh browser -- node run.mjs
//
// Environment:
//   RUN        a run directory from prepare.py (required)
//   PKG        the web node's wasm-bindgen output, from build.sh (required)
//   OUT        where results go (required)
//   BLOBS      circuit blobs, the parent of 4.0.2mainnet
//              (default /tmp/p4/gen2/circuit-blobs)
//   VERIFIER   verify-isolated-proof (default <main>/target/release/...)
//   PORT       the harness server's port (default 4229)
//   THREADS    prover threads (default 8)
//   BLOCKS     blocks to produce before the fault, or in all without one
//   FAULT      1 (default) removes the prover iframe during the next proof
//   FAULTS     2 also faults the retry
//   FAULT_PHASE genesis targets the genesis transition instead of a block
//   FAULT_KIND proof-error rejects completed requests instead of removing iframe
//   FAULT_DELAY_MS, RETRY_FAULT_DELAY_MS, RECOVERY_BLOCKS,
//   TIMEOUT_MS (default 50 minutes)
//   CHROME, PUPPETEER_DIR as for tools/isolated-prover/run.mjs
//
// Without RELAY, Chrome has a closed local proxy, disabled non-proxied UDP,
// and a dead peer. With RELAY, the caller must place Chrome and the relay in
// a namespace without an external route; an internal interface lets WebRTC
// gather a host candidate. Hostname resolution stays restricted in both modes.
// The runner owns only its Chrome and page server and closes both in `finally`.
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../..");
const sha256 = file =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const main = path.dirname(
  spawnSync(
    "git",
    ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    { cwd: repo, encoding: "utf8" },
  ).stdout.trim(),
);
const require = createRequire(
  path.join(
    process.env.PUPPETEER_DIR ??
      path.join(main, "crates/web-verifier/tests/browser"),
    "package.json",
  ),
);
const puppeteer = require("puppeteer-core");
const need = name => {
  const value = process.env[name];
  if (!value) throw new Error(`set ${name}`);
  return path.resolve(value);
};
const run = need("RUN");
const pkg = need("PKG");
const out = need("OUT");
const blobs = path.resolve(process.env.BLOBS ?? "/tmp/p4/gen2/circuit-blobs");
const port = Number(process.env.PORT ?? 4229);
const verifier = path.resolve(
  process.env.VERIFIER ??
    path.join(main, "target/release/verify-isolated-proof"),
);
const chrome =
  process.env.CHROME ??
  path.join(
    os.homedir(),
    ".local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell",
  );
const timeoutMs = Number(process.env.TIMEOUT_MS ?? 50 * 60 * 1000);
const relay = process.env.RELAY;
const paymentFile = process.env.PAYMENT_FILE;
if (
  relay &&
  !/^\/ip4\/127\.0\.0\.1\/tcp\/\d+\/webrtc\/http\/p2p\/[A-Za-z0-9]+$/.test(
    relay,
  )
)
  throw new Error("RELAY must be a loopback WebRTC address");
if (!fs.existsSync(verifier))
  throw new Error(`build the native verifier first: ${verifier}`);
const artifacts = path.join(out, "artifacts");
fs.mkdirSync(artifacts, { recursive: true });
const log = fs.createWriteStream(path.join(out, "console.log"));
const serverLog = fs.openSync(path.join(out, "server.log"), "w");
const server = spawn(
  "python3",
  [
    path.join(here, "serve.py"),
    "--pkg",
    pkg,
    "--blobs",
    blobs,
    "--run",
    run,
    "--port",
    String(port),
  ],
  { stdio: ["ignore", serverLog, serverLog] },
);
let browser;
let page;
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

const started = Date.now();
const elapsed = () => ((Date.now() - started) / 1000).toFixed(1);
try {
  for (let n = 0; ; n++) {
    if (server.exitCode !== null)
      throw new Error("server exited; see server.log");
    if (fs.readFileSync(path.join(out, "server.log"), "utf8").includes("ready"))
      break;
    if (n === 50) throw new Error("server did not start");
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-dev-shm-usage",
      ...(relay ? ["--allow-loopback-in-peer-connection"] : []),
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
      ...(relay
        ? []
        : [
            "--proxy-server=http://127.0.0.1:9",
            "--proxy-bypass-list=127.0.0.1;localhost",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
          ]),
    ],
    env: {
      ...process.env,
      LD_LIBRARY_PATH:
        process.env.LD_LIBRARY_PATH ??
        path.join(
          os.homedir(),
          ".local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu",
        ),
    },
    protocolTimeout: timeoutMs + 60000,
  });
  const loadBefore = os.loadavg();
  const startedAt = new Date().toISOString();
  page = await browser.newPage();
  const requestOrigins = new Set();
  const failedRequests = [];
  page.on("request", request => {
    try {
      requestOrigins.add(new URL(request.url()).origin);
    } catch {}
  });
  page.on("requestfailed", request => {
    failedRequests.push({
      url: request.url(),
      error: request.failure()?.errorText,
    });
  });
  page.on("console", msg => {
    log.write(`${elapsed()}s ${msg.type()}: ${msg.text()}\n`);
  });
  let fatalError;
  page.on("pageerror", error => {
    log.write(`${elapsed()}s pageerror: ${error.stack}\n`);
    // The node's thread throws this on purpose, once, to keep its worker.
    if (/Cursed hack to keep workers alive/.test(String(error?.message)))
      return;
    fatalError ??= error;
  });
  await page.exposeFunction("saveArtifact", (name, bytes) => {
    if (!/^(request|response)-\d+-\d+\.bin$/.test(name))
      throw new Error(`unexpected artifact ${name}`);
    fs.writeFileSync(path.join(artifacts, name), Buffer.from(bytes));
  });
  const cdp = await browser.target().createCDPSession();
  await page.exposeFunction("cdpWorkers", async () => {
    const workers = (await cdp.send("Target.getTargets")).targetInfos.filter(
      t => t.type === "worker",
    );
    const prover = {};
    for (const worker of workers) {
      const match = /mina-block-prover-(\d+)/.exec(worker.title ?? "");
      if (match) prover[match[1]] = (prover[match[1]] ?? 0) + 1;
    }
    return {
      total: workers.length,
      prover,
      titles: [...new Set(workers.map(w => w.title))].slice(0, 20),
    };
  });
  const query = new URLSearchParams({
    threads: process.env.THREADS ?? "8",
    blocks: process.env.BLOCKS ?? "2",
    fault: process.env.FAULT ?? "1",
    faultPhase: process.env.FAULT_PHASE ?? "block",
    faultKind: process.env.FAULT_KIND ?? "death",
    faultDelayMs: process.env.FAULT_DELAY_MS ?? "15000",
    faults: process.env.FAULTS ?? "1",
    retryFaultDelayMs: process.env.RETRY_FAULT_DELAY_MS ?? "10000",
    recoveryBlocks: process.env.RECOVERY_BLOCKS ?? "1",
    ...(relay ? { relay } : {}),
  });
  await page.goto(`http://127.0.0.1:${port}/?${query}`);
  const deadline = Date.now() + timeoutMs;
  let progress;
  let paymentInjected = false;
  let paymentInjection;
  while (Date.now() < deadline) {
    progress = await page.evaluate(() => ({
      done: window.harness?.done,
      error: window.harness?.error,
      phase: window.harness?.phase,
      last: window.harness?.status?.at(-1),
    }));
    if (progress.done || progress.error || fatalError) break;
    if (paymentFile && !paymentInjected && fs.existsSync(paymentFile)) {
      paymentInjected = true;
      try {
        const pool = await page.evaluate(() =>
          window.rpc.transaction_pool().get(),
        );
        if (pool?.length) {
          paymentInjection = { skipped: "payment already in browser pool" };
        } else {
          const payment = JSON.parse(fs.readFileSync(paymentFile, "utf8"));
          paymentInjection = await page.evaluate(
            async payment =>
              window.rpc.transaction_pool().inject().payment(payment),
            payment,
          );
        }
      } catch (error) {
        paymentInjection = { error: String(error) };
      }
    }
    await new Promise(resolve => setTimeout(resolve, 10000));
  }
  const result = await page.evaluate(() => window.harness);
  result.paymentInjection = paymentInjection ?? null;
  result.timedOut = !(result.done || result.error);
  result.pageError = fatalError ? String(fatalError.stack ?? fatalError) : null;
  result.machine = {
    cpu: os.cpus()[0].model,
    threads: os.cpus().length,
    chrome: await browser.version(),
    startedAt,
    loadBefore,
    loadAfter: os.loadavg(),
  };
  result.browserNetwork = {
    relay: relay ?? null,
    requestOrigins: [...requestOrigins].sort(),
    failedRequests,
  };
  result.artifacts = {
    commit: spawnSync("git", ["rev-parse", "HEAD"], {
      cwd: repo,
      encoding: "utf8",
    }).stdout.trim(),
    dirty: Boolean(
      spawnSync("git", ["status", "--porcelain"], {
        cwd: repo,
        encoding: "utf8",
      }).stdout.trim(),
    ),
    wasmSha256: sha256(path.join(pkg, "mina_node_web_bg.wasm")),
    verifierSha256: sha256(verifier),
    genesisSha256: sha256(path.join(run, "genesis.bin")),
    run: JSON.parse(fs.readFileSync(path.join(run, "run.json"), "utf8")),
  };
  // Every proof that came back over the bridge, checked natively.
  result.verification = [];
  for (const name of fs.readdirSync(artifacts).sort()) {
    const match = /^response-(\d+)-(\d+)\.bin$/.exec(name);
    if (!match) continue;
    const request = path.join(artifacts, `request-${match[1]}-${match[2]}.bin`);
    const verified = spawnSync(
      verifier,
      ["--bridge", request, path.join(artifacts, name)],
      {
        encoding: "utf8",
        timeout: 300000,
        env: {
          ...process.env,
          MINA_CIRCUIT_BLOBS_BASE_DIR: blobs,
          RAYON_NUM_THREADS: "8",
        },
      },
    );
    result.verification.push({
      response: name,
      sha256: sha256(path.join(artifacts, name)),
      status: verified.status,
      report:
        verified.status === 0
          ? JSON.parse(verified.stdout.trim().split("\n").at(-1))
          : null,
      stderr: verified.stderr.slice(-2000),
    });
  }
  fs.writeFileSync(
    path.join(out, "result.json"),
    JSON.stringify(result, null, 2),
  );
  const failed = result.verification.filter(v => v.status !== 0);
  console.log(
    `${elapsed()}s: done=${result.done} error=${result.error ?? null} ` +
      `proofs verified natively ${result.verification.length - failed.length}/${result.verification.length}; ` +
      `${out}/result.json`,
  );
  if (failed.length)
    throw new Error(`${failed.length} proofs failed native verification`);
  if (result.error || result.pageError)
    throw new Error(result.error ?? result.pageError);
  if (result.timedOut) throw new Error("timed out");
} catch (error) {
  fs.writeFileSync(
    path.join(out, "error.json"),
    JSON.stringify({ error: String(error.stack ?? error) }, null, 2),
  );
  throw error;
} finally {
  await cleanup();
}
