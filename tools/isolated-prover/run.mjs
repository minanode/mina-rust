#!/usr/bin/env node
// Invoke under the shared browser lock. Owns and closes both Chrome and server.
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
const out = path.resolve(process.env.OUT ?? "/tmp/i34/run");
const blobs =
  process.env.MINA_CIRCUIT_BLOBS_BASE_DIR ?? "/tmp/p4/gen2/circuit-blobs";
const chrome =
  process.env.CHROME ??
  path.join(
    os.homedir(),
    ".local/opt/chrome-headless/chrome-headless-shell/linux-155.0.8059.84/chrome-headless-shell-linux64/chrome-headless-shell",
  );
const verifier =
  process.env.VERIFIER ??
  path.join(main, "target/release/verify-isolated-proof");
if (!fs.existsSync(verifier))
  throw new Error(`build native verifier first: ${verifier}`);
fs.mkdirSync(out, { recursive: true });
const log = fs.createWriteStream(path.join(out, "console.log"));
const serverLog = fs.openSync(path.join(out, "server.log"), "w");
const server = spawn(
  "python3",
  [path.join(here, "serve.py"), "--blobs", blobs],
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

try {
  for (let n = 0; n < 50; n++) {
    if (server.exitCode !== null)
      throw new Error("server exited; see server.log");
    if (
      fs
        .readFileSync(path.join(out, "server.log"), "utf8")
        .includes("server ready")
    )
      break;
    await new Promise(resolve => setTimeout(resolve, 100));
    if (n === 49) throw new Error("server did not start");
  }
  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
    env: {
      ...process.env,
      LD_LIBRARY_PATH:
        process.env.LD_LIBRARY_PATH ??
        path.join(
          os.homedir(),
          ".local/opt/chrome-headless/libs/usr/lib/x86_64-linux-gnu",
        ),
    },
    // waitForFunction is one long-lived CDP call. Block setup plus repeated
    // proofs can exceed the old payment experiment's 120-second limit.
    protocolTimeout: Number(process.env.TIMEOUT_MS ?? 1800000),
  });
  const kind = process.env.KIND ?? "transaction";
  if (!["transaction", "block"].includes(kind))
    throw new Error("KIND must be transaction or block");
  const selectedMode =
    process.env.MODE ?? (kind === "block" ? "isolated" : "both");
  if (kind === "block" && selectedMode !== "isolated")
    throw new Error("block runs use MODE=isolated");
  if (!["both", "isolated", "control"].includes(selectedMode))
    throw new Error("MODE must be both, isolated or control");
  for (const mode of selectedMode === "both"
    ? ["isolated", "control"]
    : [selectedMode]) {
    const runOut = mode === "control" ? path.join(out, "control") : out;
    fs.mkdirSync(runOut, { recursive: true });
    const startedAt = new Date().toISOString();
    const loadBefore = os.loadavg();
    page = await browser.newPage();
    page.on("console", msg => {
      log.write(`${msg.type()}: ${msg.text()}\n`);
      console.log(msg.text());
    });
    let fatalError;
    page.on("pageerror", error => {
      log.write(`pageerror: ${error.stack}\n`);
      console.error(error);
      fatalError = error;
    });
    await page.exposeFunction("saveProof", (n, bytes) =>
      fs.writeFileSync(
        path.join(runOut, `proof-${n}.binprot`),
        Buffer.from(bytes),
      ),
    );
    await page.exposeFunction("workersRemaining", async () => {
      const cdp = await browser.target().createCDPSession();
      try {
        let count;
        for (let attempt = 0; attempt < 25; attempt++) {
          count = (await cdp.send("Target.getTargets")).targetInfos.filter(
            t => t.type === "worker",
          ).length;
          if (!count) return 0;
          await new Promise(resolve => setTimeout(resolve, 200));
        }
        return count;
      } finally {
        await cdp.detach();
      }
    });
    const threads = Number(process.env.THREADS ?? 8);
    const runs = Number(process.env.RUNS ?? (kind === "block" ? 3 : 2));
    const generations = Number(
      process.env.GENERATIONS ?? (kind === "block" ? 2 : 1),
    );
    await page.goto(
      `http://127.0.0.1:4228/${mode === "control" ? "control.html" : ""}?threads=${threads}&runs=${runs}&kind=${kind}&generations=${generations}`,
    );
    await page.waitForFunction(
      () => (window.runResult && window.teardownComplete) || window.runError,
      {
        timeout: Number(process.env.TIMEOUT_MS ?? 1800000),
        polling: 1000,
      },
    );
    const { result, error } = await page.evaluate(() => ({
      result: window.runResult,
      error: window.runError,
    }));
    if (error) throw new Error(error);
    if (fatalError) throw fatalError;
    result.mode = mode;
    if (mode === "control") await page.goto("about:blank");
    await new Promise(resolve => setTimeout(resolve, 500));
    const cdp = await browser.target().createCDPSession();
    let remainingWorkers;
    for (let attempt = 0; attempt < 25; attempt++) {
      remainingWorkers = (
        await cdp.send("Target.getTargets")
      ).targetInfos.filter(target => target.type === "worker");
      if (!remainingWorkers.length) break;
      await new Promise(resolve => setTimeout(resolve, 200));
    }
    result.workersAfterHostRemoval = remainingWorkers.length;
    result.remainingWorkers = remainingWorkers;
    await cdp.detach();
    result.machine = {
      cpu: os.cpus()[0].model,
      chrome: await browser.version(),
      startedAt,
      loadBefore,
      loadAfter: os.loadavg(),
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
      wasmSha256: sha256(
        path.join(here, "www/pkg/mina_isolated_prover_bg.wasm"),
      ),
      verifierSha256: sha256(verifier),
      fixtureSha256: sha256(
        path.join(
          repo,
          "crates/ledger/src/proofs/data",
          kind === "block"
            ? "mesa_mainnet_genesis_block_input.bin"
            : "mesa_mainnet_payment_base_job.bin",
        ),
      ),
    };
    result.verification = [];
    for (let n = 0; n < (result.proofCount ?? runs); n++) {
      const verified = spawnSync(
        verifier,
        [
          ...(kind === "block" ? ["--block"] : []),
          path.join(
            repo,
            kind === "block"
              ? "crates/ledger/src/proofs/data/mesa_mainnet_genesis_block_input.bin"
              : "crates/ledger/src/proofs/data/mesa_mainnet_payment_base_job.bin",
          ),
          path.join(runOut, `proof-${n}.binprot`),
        ],
        {
          encoding: "utf8",
          timeout: 180000,
          env: {
            ...process.env,
            MINA_CIRCUIT_BLOBS_BASE_DIR: blobs,
            RAYON_NUM_THREADS: "8",
          },
        },
      );
      result.verification.push({
        proofSha256: sha256(path.join(runOut, `proof-${n}.binprot`)),
        status: verified.status,
        stdout: verified.stdout,
        stderr: verified.stderr,
      });
      if (verified.status !== 0) {
        fs.writeFileSync(
          path.join(runOut, "result.json"),
          JSON.stringify(result, null, 2),
        );
        throw new Error(`native verification failed: ${verified.stderr}`);
      }
    }
    fs.writeFileSync(
      path.join(runOut, "result.json"),
      JSON.stringify(result, null, 2),
    );
    console.log(
      `Verified ${result.verification.length} proofs; measurements: ${runOut}/result.json`,
    );
    if (result.workersAfterHostRemoval !== 0)
      throw new Error(
        `prover teardown left ${result.workersAfterHostRemoval} workers alive`,
      );
    await page.close();
  }
} catch (error) {
  const progress =
    page && (await page.evaluate(() => window.runProgress).catch(() => null));
  fs.writeFileSync(
    path.join(out, "error.json"),
    JSON.stringify({ error: String(error.stack ?? error), progress }, null, 2),
  );
  throw error;
} finally {
  await cleanup();
}
