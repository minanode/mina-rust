// Open the web node frontend in headless Chrome, start a node without a block
// producer, and every EVERY_MS print the browser's memory and the node's
// status. See ../README.md.
//
//   CHROME=... BASE=http://localhost:4202 node run.mjs [minutes]
//   CHROME=... BASE=http://localhost:4202 node run.mjs --dry-run
//   node run.mjs --help
//
// It runs for [minutes] (default 30), or until UNTIL_SYNCED seconds after the
// node first reports Synced, whichever comes first. --dry-run checks CHROME,
// BASE, puppeteer-core and the browser lock, without starting a browser.
//
// Before starting its browser it takes the shared machine's browser lock
// (../README.md, "Taking turns"), waiting while another run holds it, and
// holds it until it exits.
//
// Environment:
//   CHROME        the Chrome or chrome-headless-shell binary (required)
//   BASE          the frontend's origin, e.g. http://localhost:4210 (required)
//   PAGE          the path to open (default /loading-web-node)
//   BUTTON        the button that starts the node (default "Start Non BP")
//   EVERY_MS      the sampling interval (default 30000)
//   POLL_MS       also call status() from the page every POLL_MS, at most one
//                 call in flight and without a timeout, and log each answer's
//                 latency to the console as `RPCPOLL status <ms> <sync>
//                 <best tip> <connected peers>`; a call still waiting when the
//                 run ends is logged as `RPCPOLL status pending <ms>`
//   FLOOD         from FLOOD_FROM to FLOOD_TO seconds (default 90 and 150)
//                 after the page is opened, run FLOOD loops in the page that
//                 each call status(), or state().get(FLOOD_FILTER) with
//                 FLOOD_FILTER, again as soon as the previous call answers,
//                 like many HTTP clients polling a native node; the page logs
//                 `RPCFLOOD done=<answers> errors=<errors>` every 5 s and at
//                 the stop
//   UNTIL_SYNCED  stop this many seconds after the node first reports Synced
//   CHROME_ARGS   more Chrome flags, split before each `--`, such as
//                 --host-resolver-rules=... and
//                 --ignore-certificate-errors-spki-list=... for a test site
//                 on names and a CA of its own (tools/deploy/test)
//   ICE_POLICY    `relay` to make every RTCPeerConnection of the page use
//                 TURN only (iceTransportPolicy), to test a TURN server; a
//                 test switch, the node itself never sets it
//   STOP_FILE     stop cleanly when this file appears (checked each sample)
//   OUT           where to write the files below (default the current dir)
//   LOCK_WAIT     give up after waiting this many seconds for the browser lock
//                 (default: wait as long as it takes; 0: don't wait)
//   MINA_LOCK_DIR where the lock files are (default the main checkout's
//                 .work/locks)
//   MINA_NO_LOCK  1 to run without the lock, on a machine you have to yourself
//
// Writes to OUT:
//   run.log       what it prints: `<s>s rss=<MB>MB wasm=[<MB>]MB status={...}
//                 rtc={...}`, rtc being the page's WebRTC connections: how
//                 many are open, the MB received and sent over all it made,
//                 and the local and remote candidate types of each open one
//   console.log   the page's console, each line prefixed with seconds since
//                 the page was opened, and the status and URL of each
//                 response from another origin
//   browser.ws    the browser's DevTools endpoint, for RUN_DIR=OUT in the
//                 attach scripts
//   final.png     a screenshot when it stops
import { accessSync, appendFileSync, constants, existsSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { execSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ENDPOINT_FILE, findPuppeteer, outDir, release, takeLock } from "./lib.mjs";

const args = process.argv.slice(2);
if (args.includes("--help") || args.includes("-h")) {
  // The comment at the top of this file.
  const lines = readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n");
  const header = lines.slice(0, lines.findIndex((line) => !line.startsWith("//")));
  console.log(header.map((line) => line.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(0);
}
const dryRun = args.includes("--dry-run");
const unknown = args.filter((a) => a.startsWith("-") && a !== "--dry-run");
if (unknown.length) {
  console.error(`unknown option ${unknown.join(" ")}; see --help`);
  process.exit(2);
}
const minutes = Number(args.find((a) => !a.startsWith("-")) ?? 30);
const base = process.env.BASE;
const path = process.env.PAGE ?? "/loading-web-node";
const out = outDir();
const untilSynced = process.env.UNTIL_SYNCED === undefined ? null : Number(process.env.UNTIL_SYNCED);
const stopFile = process.env.STOP_FILE;
if (Number.isNaN(minutes) || Number.isNaN(untilSynced)) {
  console.error("[minutes] and UNTIL_SYNCED are numbers; see --help");
  process.exit(2);
}
const missing = ["CHROME", "BASE"].filter((name) => !process.env[name]);
if (missing.length && !dryRun) {
  console.error(`set ${missing.join(" and ")}; see --help and tools/regression/README.md`);
  process.exit(2);
}

if (dryRun) process.exit(await dryRunChecks());

let puppeteer, lock;
try {
  ({ puppeteer } = findPuppeteer());
  lock = await takeLock(["browser"], `run.mjs, BASE=${base}${path}, OUT=${out}`);
} catch (error) {
  console.error(error.message);
  process.exit(error.code === 75 ? 75 : 1);
}

const consoleLog = join(out, "console.log");
const runLog = join(out, "run.log");
mkdirSync(out, { recursive: true });
writeFileSync(consoleLog, "");
writeFileSync(runLog, "");

const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  args: [
    "--no-sandbox",
    "--disable-gpu",
    "--js-flags=--stack-trace-limit=60",
    // Split before each flag only: a value may hold spaces.
    ...(process.env.CHROME_ARGS ?? "")
      .trim()
      .split(/\s+(?=--)/)
      .filter(Boolean),
  ],
  defaultViewport: { width: 1600, height: 1000 },
  protocolTimeout: 90000,
});
writeFileSync(join(out, ENDPOINT_FILE), browser.wsEndpoint() + "\n");
const started = Date.now();
const elapsed = () => `${Math.round((Date.now() - started) / 1000)}s`;
const log = (line) => {
  console.log(line);
  appendFileSync(runLog, line + "\n");
};

// Resident memory of this browser and all its descendant processes, in MB.
function browserRss() {
  const pids = [String(browser.process().pid)];
  for (let i = 0; i < pids.length; i++) {
    try {
      pids.push(...execSync(`pgrep -P ${pids[i]}`).toString().trim().split(/\s+/).filter(Boolean));
    } catch {}
  }
  const kb = pids
    .map((p) => {
      try {
        return Number(execSync(`ps -o rss= -p ${p}`).toString().trim()) || 0;
      } catch {
        return 0;
      }
    })
    .reduce((a, b) => a + b, 0);
  return Math.round(kb / 1024);
}

try {
  const page = await browser.newPage();
  page.on("console", (m) => appendFileSync(consoleLog, `${elapsed()} [${m.type()}] ${m.text()}\n`));
  page.on("pageerror", (err) => appendFileSync(consoleLog, `${elapsed()} [pageerror] ${err.stack ?? err.message}\n`));
  page.on("requestfailed", (r) =>
    appendFileSync(consoleLog, `${elapsed()} [requestfailed] ${r.url()} ${r.failure()?.errorText}\n`),
  );
  // Responses from other origins (an asset host, a relay's signalling), so a
  // run shows where the page's files came from. URLs are cut: a signalling
  // URL carries the whole offer.
  page.on("response", (r) => {
    try {
      if (new URL(r.url()).origin !== new URL(base).origin) {
        appendFileSync(consoleLog, `${elapsed()} [response] ${r.status()} ${r.url().slice(0, 120)}\n`);
      }
    } catch {}
  });
  // Keep the WASM memories so their size can be read.
  await page.evaluateOnNewDocument(() => {
    const Memory = WebAssembly.Memory;
    WebAssembly.Memory = function (desc) {
      const memory = new Memory(desc);
      (self.__wasmMemories ||= []).push(memory);
      return memory;
    };
    WebAssembly.Memory.prototype = Memory.prototype;
  });
  // Keep the WebRTC connections, to read their traffic and candidates; with
  // ICE_POLICY=relay, make them use TURN only.
  await page.evaluateOnNewDocument((policy) => {
    const Native = RTCPeerConnection;
    const peers = (self.__rtcPeers = []);
    self.RTCPeerConnection = function (config) {
      const pc = new Native(policy ? { ...config, iceTransportPolicy: policy } : config);
      peers.push(pc);
      return pc;
    };
    self.RTCPeerConnection.prototype = Native.prototype;
    Object.setPrototypeOf(self.RTCPeerConnection, Native);
  }, process.env.ICE_POLICY || null);
  const pollMs = Number(process.env.POLL_MS ?? 0);
  if (pollMs > 0) {
    // Time every status() call from the page itself, as the frontend makes
    // them, at most one in flight, so that a slow answer shows as such
    // instead of as a timeout.
    await page.evaluateOnNewDocument((pollMs) => {
      const poll = (self.__rpcPoll = { since: null });
      setInterval(() => {
        const node = self.webnode;
        if (!node?.status || poll.since !== null) return;
        const t0 = (poll.since = performance.now());
        Promise.resolve()
          .then(() => node.status())
          .then(
            (s) => {
              const tf = s?.transition_frontier;
              const sync = tf?.sync?.status ?? "";
              const peers = Array.isArray(s?.peers)
                ? s.peers.filter((p) => p.connection_status === "Connected").length
                : "";
              console.log(
                `RPCPOLL status ${Math.round(performance.now() - t0)} ${sync} ${tf?.best_tip?.height ?? ""} ${peers}`,
              );
            },
            (e) => console.log(`RPCPOLL status error ${Math.round(performance.now() - t0)} ${e}`),
          )
          .finally(() => (poll.since = null));
      }, pollMs);
    }, pollMs);
  }
  const floodN = Number(process.env.FLOOD ?? 0);
  if (floodN > 0) {
    // Callers that ask again as soon as they are answered, as I6 and R3 used
    // to load the node thread with requests.
    await page.evaluateOnNewDocument(() => {
      const flood = (self.__rpcFlood = { on: false, done: 0, errors: 0 });
      self.__rpcFloodStart = (n, filter) => {
        flood.on = true;
        const loop = () => {
          if (!flood.on) return;
          Promise.resolve()
            .then(() => (filter ? self.webnode.state().get(filter) : self.webnode.status()))
            .then(
              () => flood.done++,
              () => flood.errors++,
            )
            .then(loop);
        };
        for (let i = 0; i < n; i++) loop();
        console.log(`RPCFLOOD start ${n} ${filter ?? "status"}`);
      };
      self.__rpcFloodStop = () => {
        flood.on = false;
        console.log(`RPCFLOOD stop done=${flood.done} errors=${flood.errors}`);
      };
      setInterval(() => {
        if (flood.on) console.log(`RPCFLOOD done=${flood.done} errors=${flood.errors}`);
      }, 5000);
    });
    const at = (env, fallback) => Number(process.env[env] ?? fallback) * 1000 - (Date.now() - started);
    const filter = process.env.FLOOD_FILTER ?? null;
    setTimeout(() => {
      log(`${elapsed()} flood start: ${floodN} loops on ${filter ?? "status()"}`);
      page.evaluate((n, f) => self.__rpcFloodStart(n, f), floodN, filter).catch((e) => log(`flood: ${e.message}`));
    }, Math.max(0, at("FLOOD_FROM", 90))).unref();
    setTimeout(() => {
      log(`${elapsed()} flood stop`);
      page.evaluate(() => self.__rpcFloodStop()).catch((e) => log(`flood: ${e.message}`));
    }, Math.max(0, at("FLOOD_TO", 150))).unref();
  }
  await page.goto(base + path, { waitUntil: "domcontentloaded", timeout: 120000 });
  const isolated = await page.evaluate(() => self.crossOriginIsolated);
  log(`${elapsed()} loaded ${base + path}, crossOriginIsolated=${isolated}`);
  if (!isolated) log(`${elapsed()} warning: not cross-origin isolated, so no threads; serve with serve.json`);
  // The setup screen: start without a block producer.
  const button = process.env.BUTTON ?? "Start Non BP";
  await page.waitForFunction(
    (label) => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === label),
    { timeout: 60000 },
    button,
  );
  await page.evaluate((label) => {
    [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === label).click();
  }, button);
  log(`${elapsed()} clicked ${button}`);

  let syncedAt = null;
  while (Date.now() - started < minutes * 60000) {
    if (stopFile && existsSync(stopFile)) break;
    if (syncedAt !== null && Date.now() - syncedAt >= untilSynced * 1000) break;
    await new Promise((r) => setTimeout(r, Number(process.env.EVERY_MS ?? 30000)));
    const status = await page
      .evaluate(async () => {
        const node = window.webnode;
        if (!node?.status) return null;
        const timeout = new Promise((r) => setTimeout(() => r("timeout"), 10000));
        const s = await Promise.race([node.status(), timeout]);
        if (s === "timeout") return { error: "status timed out" };
        const tf = s?.transition_frontier;
        return {
          best_tip: tf?.best_tip?.height ?? null,
          sync: tf?.sync?.status ?? tf?.sync ?? null,
          peers: Array.isArray(s?.peers) ? s.peers.filter((p) => p.connection_status === "Connected").length : null,
        };
      })
      .catch((e) => ({ error: e.message }));
    const wasm = await page
      .evaluate(() => (self.__wasmMemories ?? []).map((m) => Math.round(m.buffer.byteLength / 2 ** 20)))
      .catch(() => null);
    const rtc = await page
      .evaluate(async () => {
        const result = { open: 0, rx_mb: 0, tx_mb: 0, pairs: [] };
        for (const pc of self.__rtcPeers ?? []) {
          const stats = await pc.getStats().catch(() => null);
          if (!stats) continue;
          const byId = new Map([...stats.values()].map((s) => [s.id, s]));
          for (const s of stats.values()) {
            if (s.type !== "transport") continue;
            result.rx_mb += (s.bytesReceived ?? 0) / 2 ** 20;
            result.tx_mb += (s.bytesSent ?? 0) / 2 ** 20;
            const pair = byId.get(s.selectedCandidatePairId);
            if (pc.connectionState !== "connected" || !pair) continue;
            result.open++;
            const local = byId.get(pair.localCandidateId);
            const remote = byId.get(pair.remoteCandidateId);
            result.pairs.push(`${local?.candidateType}>${remote?.candidateType}:${remote?.address}:${remote?.port}`);
          }
        }
        result.rx_mb = Math.round(result.rx_mb * 10) / 10;
        result.tx_mb = Math.round(result.tx_mb * 10) / 10;
        return result;
      })
      .catch((e) => ({ error: e.message }));
    log(
      `${elapsed()} rss=${browserRss()}MB wasm=${JSON.stringify(wasm)}MB status=${JSON.stringify(status)?.slice(0, 400)} rtc=${JSON.stringify(rtc)}`,
    );
    if (syncedAt === null && untilSynced !== null && status?.sync === "Synced") syncedAt = Date.now();
  }
  if (pollMs > 0) {
    await page
      .evaluate(() => {
        const since = self.__rpcPoll?.since;
        if (since !== null && since !== undefined) {
          console.log(`RPCPOLL status pending ${Math.round(performance.now() - since)}`);
        }
      })
      .catch(() => {});
  }
  await page.screenshot({ path: join(out, "final.png") });
} finally {
  await browser.close();
  rmSync(join(out, ENDPOINT_FILE), { force: true });
  release(lock);
}

// What --dry-run checks: everything a run needs before its browser starts.
// Returns the exit code: 0 if a run could start now.
async function dryRunChecks() {
  let ok = true;
  const report = (good, line) => {
    console.log(`${good ? "ok  " : "FAIL"} ${line}`);
    ok &&= good;
  };
  if (!process.env.CHROME) report(false, "CHROME is not set");
  else {
    try {
      accessSync(process.env.CHROME, constants.X_OK);
      report(true, `CHROME ${process.env.CHROME} is executable`);
    } catch (error) {
      report(false, `CHROME ${process.env.CHROME}: ${error.code}`);
    }
  }
  try {
    report(true, `puppeteer-core found from ${findPuppeteer().dir}`);
  } catch (error) {
    report(false, error.message);
  }
  if (!base) report(false, "BASE is not set");
  else {
    try {
      const response = await fetch(base + path, { signal: AbortSignal.timeout(10000) });
      const coop = response.headers.get("cross-origin-opener-policy");
      const coep = response.headers.get("cross-origin-embedder-policy");
      report(response.ok, `GET ${base + path}: ${response.status}`);
      // Without both the page is not cross-origin isolated, and has no threads.
      report(coop === "same-origin" && coep === "require-corp", `COOP ${coop}, COEP ${coep}`);
    } catch (error) {
      report(false, `GET ${base + path}: ${error.cause?.message ?? error.message}`);
    }
  }
  // A dry run doesn't wait for the lock unless LOCK_WAIT says to. A lock that
  // is taken is not a failure: a run would wait for it.
  const note = (line) => console.log(`     ${line}`);
  try {
    const wait = process.env.LOCK_WAIT ? undefined : 0;
    const held = await takeLock(["browser"], `run.mjs --dry-run, OUT=${out}`, { log: note, wait });
    if (!held) report(true, "no browser lock (MINA_NO_LOCK=1)");
    else if (held.fd === null) report(true, "the browser lock is held by the process that started this one");
    else report(true, `took and released the browser lock, ${held.path}`);
    release(held);
  } catch (error) {
    if (error.code !== 75) report(false, error.message);
    else note(`the browser lock is taken, so a run would wait for it; taken by:\n${error.holders}`);
  }
  note(`a run would write run.log, console.log, browser.ws and final.png to ${out}${existsSync(out) ? "" : ", made when it starts"}`);
  return ok ? 0 : 1;
}
