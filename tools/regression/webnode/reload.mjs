// The web node's frontier store across reloads (I5e), in headless Chrome:
// bootstrap with the store on, wait for its first checkpoint, reload and time
// the restore to the network's tip, then damage the store and check that the
// next start fails closed and bootstraps. See ../README.md and
// private implementation notes
//
//   CHROME=... BASE=http://localhost:4210 REFERENCE=http://host:3540/graphql \
//     node reload.mjs
//
// One browser profile for the whole run, so the origin's Origin Private File
// System (OPFS) lasts from one page load to the next, as a visitor's does.
// Each "reload" opens the page anew (a new document, new workers), and starts
// the node again with the same button.
//
// Phases, each one a line in phases.jsonl:
//   bootstrap  a cold start with an empty store; until synced at the tip,
//              then until the first checkpoint is written (CHECKPOINT_WAIT)
//   reload     RELOADS times: a reload, timed to synced at the tip
//   corrupt    a byte flipped in the largest stored ledger file, then a
//              start: the store must fail its check and the node bootstrap
//   tab        (TAB=1) a second tab while the first runs: its store must be
//              locked out, and its node run without one
//   quota      (QUOTA_MB) last: an empty store, the origin's quota set to
//              QUOTA_MB through DevTools, a start: the store must be turned
//              off by the full quota and the node keep following
//   soak       (SOAK_MINUTES) the last reloaded page kept open, the store's
//              size read every minute: with CHECKPOINT_SECS short, it shows
//              that checkpoints replace each other instead of piling up
//
// "Synced at the tip": the node's sync status is Synced, its best tip is not
// the genesis block, and its height is at least the reference node's best tip
// height minus 1 (REFERENCE, a GraphQL URL, read each sample).
//
// Environment:
//   CHROME, BASE    as run.mjs (required)
//   REFERENCE       GraphQL URL of a node at the tip (required)
//   PAGE, BUTTON    as run.mjs (BUTTON default: "Start the web node", or else
//                   "Start Non BP")
//   SAMPLE_MS       sampling interval (default 1000)
//   PHASE_MINUTES   longest wait for one phase (default 20)
//   CHECKPOINT_WAIT seconds to wait for the first checkpoint after the
//                   bootstrap's sync (default 600)
//   CHECKPOINT_SECS localStorage webnodeCheckpointSecs: seconds between
//                   checkpoints after the first (default unset: 30 minutes)
//   RELOADS         reloads in the reload phase (default 3)
//   FOLLOW_SECS     how long each phase follows after synced at the tip
//                   (default 20)
//   SKIP            comma-separated phases to skip (bootstrap, reload, corrupt)
//   SOAK_MINUTES    minutes of the soak phase (default 0: none)
//   QUOTA_MB        the quota phase's quota (default 0: no quota phase)
//   TAB             1 for the second tab phase
//   OUT             output directory (default the current one)
//   PROFILE_DIR     the browser profile, deleted at the end (default
//                   OUT/profile): it holds the store, so keep it out of the
//                   evidence
//   LOCK_WAIT, MINA_LOCK_DIR, MINA_NO_LOCK  as run.mjs
//
// Writes to OUT: run.log, console.log (every page's console, with its phase),
// samples.jsonl (one sample per SAMPLE_MS: phase, seconds since the phase's
// navigation, sync status, best tip, reference height, WASM memories, browser
// RSS), phases.jsonl (one summary per phase), opfs-*.json (the store's files
// after each phase) and a screenshot per phase.
import { appendFileSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { execSync } from "node:child_process";
import { join } from "node:path";
import { ENDPOINT_FILE, findPuppeteer, outDir, release, takeLock } from "./lib.mjs";

const env = (name, fallback) => process.env[name] ?? fallback;
const base = env("BASE");
const path = env("PAGE", "/loading-web-node");
const reference = env("REFERENCE");
const sampleMs = Number(env("SAMPLE_MS", 1000));
const phaseMs = Number(env("PHASE_MINUTES", 20)) * 60000;
const checkpointWait = Number(env("CHECKPOINT_WAIT", 600)) * 1000;
const reloads = Number(env("RELOADS", 3));
const followMs = Number(env("FOLLOW_SECS", 20)) * 1000;
const skip = new Set(env("SKIP", "").split(",").filter(Boolean));
const buttons = process.env.BUTTON ? [process.env.BUTTON] : ["Start the web node", "Start Non BP"];
const out = outDir();
const missing = ["CHROME", "BASE", "REFERENCE"].filter((name) => !process.env[name]);
if (missing.length) {
  console.error(`set ${missing.join(", ")}; see the comment at the top`);
  process.exit(2);
}

mkdirSync(out, { recursive: true });
for (const file of ["run.log", "console.log", "samples.jsonl", "phases.jsonl"]) writeFileSync(join(out, file), "");
const started = Date.now();
const log = (line) => {
  const text = `${new Date().toISOString().slice(11, 19)} ${line}`;
  console.log(text);
  appendFileSync(join(out, "run.log"), text + "\n");
};
const jsonl = (file, value) => appendFileSync(join(out, file), JSON.stringify(value) + "\n");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const { puppeteer } = findPuppeteer();
let lock;
try {
  lock = await takeLock(["browser"], `reload.mjs, BASE=${base}${path}, OUT=${out}`);
} catch (error) {
  console.error(error.message);
  process.exit(error.code === 75 ? 75 : 1);
}

const profile = env("PROFILE_DIR", join(out, "profile"));
rmSync(profile, { recursive: true, force: true });
const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  userDataDir: profile,
  args: ["--no-sandbox", "--disable-gpu", "--js-flags=--stack-trace-limit=60"],
  defaultViewport: { width: 1600, height: 1000 },
  protocolTimeout: 120000,
});
writeFileSync(join(out, ENDPOINT_FILE), browser.wsEndpoint() + "\n");

function browserRss() {
  const pids = [String(browser.process().pid)];
  for (let i = 0; i < pids.length; i++) {
    try {
      pids.push(...execSync(`pgrep -P ${pids[i]}`).toString().trim().split(/\s+/).filter(Boolean));
    } catch {}
  }
  let kb = 0;
  for (const pid of pids) {
    try {
      kb += Number(execSync(`ps -o rss= -p ${pid}`).toString().trim()) || 0;
    } catch {}
  }
  return Math.round(kb / 1024);
}

async function referenceHeight() {
  try {
    const response = await fetch(reference, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ query: "{ bestChain(maxLength: 1) { protocolState { consensusState { blockHeight } } } }" }),
      signal: AbortSignal.timeout(5000),
    });
    const body = await response.json();
    return Number(body?.data?.bestChain?.[0]?.protocolState?.consensusState?.blockHeight) || null;
  } catch {
    return null;
  }
}

// The page's console, with the phase it belongs to.
let phase = "setup";
let phaseStart = Date.now();
const consoleLines = [];
function watch(page, tab) {
  page.on("console", (m) => {
    const line = `${((Date.now() - phaseStart) / 1000).toFixed(1)}s ${phase}${tab ? `/${tab}` : ""} [${m.type()}] ${m.text()}`;
    appendFileSync(join(out, "console.log"), line + "\n");
    consoleLines.push({ phase, tab, at: Date.now() - phaseStart, text: m.text() });
  });
  page.on("pageerror", (err) => appendFileSync(join(out, "console.log"), `${phase} [pageerror] ${err.stack ?? err.message}\n`));
}

async function newPage(tab) {
  const page = await browser.newPage();
  watch(page, tab);
  await page.evaluateOnNewDocument(() => {
    const Memory = WebAssembly.Memory;
    WebAssembly.Memory = function (desc) {
      const memory = new Memory(desc);
      (self.__wasmMemories ||= []).push(memory);
      return memory;
    };
    WebAssembly.Memory.prototype = Memory.prototype;
  });
  return page;
}

// The store's files in this origin's OPFS, from any page of the origin.
async function opfsFiles(page) {
  return page.evaluate(async () => {
    const files = [];
    const walk = async (dir, prefix) => {
      for await (const [name, handle] of dir.entries()) {
        if (handle.kind === "directory") await walk(handle, `${prefix}${name}/`);
        else {
          const file = await handle.getFile();
          files.push({ path: `${prefix}${name}`, bytes: file.size, modified: file.lastModified });
        }
      }
    };
    const root = await navigator.storage.getDirectory();
    await walk(root, "");
    const estimate = await navigator.storage.estimate();
    return { files, usage: estimate.usage, quota: estimate.quota };
  });
}

async function start(page) {
  await page.goto(base + path, { waitUntil: "domcontentloaded", timeout: 120000 });
  const isolated = await page.evaluate(() => self.crossOriginIsolated);
  if (!isolated) log("warning: the page is not cross-origin isolated");
  await page.evaluate((secs) => {
    localStorage.setItem("webnodeFrontierStore", "on");
    if (secs) localStorage.setItem("webnodeCheckpointSecs", String(secs));
  }, env("CHECKPOINT_SECS", ""));
  const label = await page.waitForFunction(
    (labels) => {
      const button = [...document.querySelectorAll("button")].find((b) => labels.includes(b.innerText.trim()));
      return button ? button.innerText.trim() : false;
    },
    { timeout: 120000 },
    buttons,
  );
  const text = await label.jsonValue();
  await page.evaluate((text) => {
    [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === text).click();
  }, text);
  return text;
}

async function sample(page) {
  const status = await page
    .evaluate(async () => {
      const node = window.webnode;
      if (!node?.status) return null;
      const timeout = new Promise((r) => setTimeout(() => r("timeout"), 10000));
      const s = await Promise.race([node.status(), timeout]);
      if (s === "timeout") return { error: "status timed out" };
      const tf = s?.transition_frontier;
      return {
        sync: tf?.sync?.status ?? null,
        best_tip: tf?.best_tip?.height ?? null,
        best_tip_hash: tf?.best_tip?.hash ?? null,
        peers: Array.isArray(s?.peers) ? s.peers.filter((p) => p.connection_status === "Connected").length : null,
      };
    })
    .catch((e) => ({ error: e.message }));
  const wasm = await page
    .evaluate(() => (self.__wasmMemories ?? []).map((m) => Math.round(m.buffer.byteLength / 2 ** 20)))
    .catch(() => null);
  return { status, wasm };
}

function atTip(status, ref) {
  if (status?.sync !== "Synced" || status.best_tip === null || ref === null) return false;
  return status.best_tip >= ref - 1;
}

// Samples until `done(sample)` holds (or the phase's time is up), keeping the
// peak WASM memory. Returns the time it held and the peak.
async function until(page, done, what, { limit = phaseMs } = {}) {
  let peak = 0;
  let last = null;
  const deadline = Date.now() + limit;
  while (Date.now() < deadline) {
    const { status, wasm } = await sample(page);
    const ref = await referenceHeight();
    const at = (Date.now() - phaseStart) / 1000;
    peak = Math.max(peak, ...(wasm ?? [0]));
    const record = { phase, at, status, reference: ref, wasm, rss: browserRss() };
    jsonl("samples.jsonl", record);
    last = record;
    if (done(record)) return { at, peak, last };
    await sleep(sampleMs);
  }
  log(`${phase}: gave up waiting for ${what} after ${limit / 1000} s; last ${JSON.stringify(last)}`);
  return { at: null, peak, last };
}

const persistLines = () =>
  consoleLines.filter((l) => l.phase === phase && !l.tab && /Persist|frontier store/.test(l.text)).map((l) => `${(l.at / 1000).toFixed(1)}s ${l.text}`);
const sawLine = (pattern) => consoleLines.some((l) => l.phase === phase && !l.tab && pattern.test(l.text));

async function runPhase(name, page, { expect, waitCheckpoint = false } = {}) {
  phase = name;
  phaseStart = Date.now();
  log(`${name}: open ${base + path}`);
  const button = await start(page);
  log(`${name}: clicked "${button}" ${((Date.now() - phaseStart) / 1000).toFixed(1)} s after navigation`);
  const synced = await until(page, (r) => atTip(r.status, r.reference), "synced at the tip");
  log(`${name}: synced at the tip at ${synced.at} s, best tip ${synced.last?.status?.best_tip}, reference ${synced.last?.reference}, peak WASM ${synced.peak} MiB`);
  let checkpoint = null;
  if (waitCheckpoint && synced.at !== null) {
    const written = await until(page, () => sawLine(/checkpoint written|CheckpointSuccess/), "the first checkpoint", { limit: checkpointWait });
    checkpoint = written.at;
    log(`${name}: checkpoint written by ${checkpoint} s`);
  }
  const followed = await until(page, () => false, "following", { limit: followMs });
  const store = await opfsFiles(page).catch((e) => ({ error: e.message }));
  writeFileSync(join(out, `opfs-${name}.json`), JSON.stringify(store, null, 2));
  await page.screenshot({ path: join(out, `${name}.png`) }).catch(() => {});
  const summary = {
    phase: name,
    synced_at_tip_s: synced.at,
    checkpoint_s: checkpoint,
    peak_wasm_mib: Math.max(synced.peak, followed.peak),
    peak_wasm_to_sync_mib: synced.peak,
    best_tip: synced.last?.status?.best_tip ?? null,
    reference: synced.last?.reference ?? null,
    opfs_bytes: store.files?.filter((f) => f.path.startsWith("mina-frontier/")).reduce((a, f) => a + f.bytes, 0) ?? null,
    opfs_files: store.files?.filter((f) => f.path.startsWith("mina-frontier/")).length ?? null,
    usage: store.usage ?? null,
    quota: store.quota ?? null,
    persist: persistLines(),
    expect,
  };
  jsonl("phases.jsonl", summary);
  log(`${name}: ${JSON.stringify({ ...summary, persist: summary.persist.length })}`);
  return summary;
}

// Flips one byte in the middle of the largest stored ledger file, from a
// page of the origin with no web node running (its workers would hold the
// files). The manifest still names the file with its old checksum.
async function corruptLedger(page) {
  await page.goto(base + "/build-info.json", { waitUntil: "domcontentloaded" });
  await sleep(2000);
  return page.evaluate(async () => {
    const stores = await (await navigator.storage.getDirectory()).getDirectoryHandle("mina-frontier");
    let largest = null;
    for await (const [chain, dir] of stores.entries()) {
      if (dir.kind !== "directory") continue;
      for await (const [name, handle] of dir.entries()) {
        if (!name.startsWith("ledger-")) continue;
        const file = await handle.getFile();
        if (!largest || file.size > largest.size) largest = { chain, name, handle, size: file.size };
      }
    }
    if (!largest) return null;
    const at = Math.floor(largest.size / 2);
    const byte = new Uint8Array(await (await largest.handle.getFile()).slice(at, at + 1).arrayBuffer());
    const writable = await largest.handle.createWritable({ keepExistingData: true });
    await writable.write({ type: "write", position: at, data: new Uint8Array([byte[0] ^ 0xff]) });
    await writable.close();
    return { chain: largest.chain, file: largest.name, bytes: largest.size, at };
  });
}

let exit = 0;
try {
  let page = await newPage();
  if (!skip.has("bootstrap")) {
    // An empty store first.
    await page.goto(base + "/build-info.json", { waitUntil: "domcontentloaded" });
    await page.evaluate(async () => {
      const root = await navigator.storage.getDirectory();
      await root.removeEntry("mina-frontier", { recursive: true }).catch(() => {});
    });
    await runPhase("bootstrap", page, { expect: "bootstrap, then a checkpoint", waitCheckpoint: true });
  }
  if (!skip.has("reload")) {
    for (let i = 1; i <= reloads; i++) {
      // A new document: the old page's workers are torn down.
      await page.close();
      page = await newPage();
      await runPhase(`reload-${i}`, page, { expect: "restored" });
    }
  }
  if (process.env.TAB === "1") {
    phase = "tab";
    phaseStart = Date.now();
    const second = await newPage("second");
    await start(second);
    await until(second, () => consoleLines.some((l) => l.tab === "second" && /locked|another tab/.test(l.text)), "the second tab's store locked out", { limit: 120000 });
    jsonl("phases.jsonl", {
      phase: "tab",
      lines: consoleLines.filter((l) => l.tab === "second" && /Persist|frontier store|locked/.test(l.text)).map((l) => l.text),
    });
    await second.close();
  }
  const soakMinutes = Number(env("SOAK_MINUTES", 0));
  if (soakMinutes > 0) {
    phase = "soak";
    phaseStart = Date.now();
    log(`soak: ${soakMinutes} minutes on the open page`);
    const writtenBefore = consoleLines.filter((l) => !l.tab && /checkpoint written/.test(l.text)).length;
    for (let minute = 1; minute <= soakMinutes; minute++) {
      await until(page, () => false, "soaking", { limit: 60000 });
      const store = await opfsFiles(page).catch((e) => ({ error: e.message }));
      const files = store.files?.filter((f) => f.path.startsWith("mina-frontier/")) ?? [];
      const record = {
        phase: "soak",
        minute,
        checkpoints: consoleLines.filter((l) => !l.tab && /checkpoint written/.test(l.text)).length - writtenBefore,
        opfs_bytes: files.reduce((a, f) => a + f.bytes, 0),
        opfs_files: files.length,
        ledger_files: files.filter((f) => /\/ledger-/.test(f.path)).length,
        usage: store.usage ?? null,
      };
      jsonl("phases.jsonl", record);
      log(`soak: ${JSON.stringify(record)}`);
    }
  }
  if (!skip.has("corrupt")) {
    await page.close();
    page = await newPage();
    phase = "corrupt";
    phaseStart = Date.now();
    const damaged = await corruptLedger(page);
    log(`corrupt: flipped ${JSON.stringify(damaged)}`);
    jsonl("phases.jsonl", { phase: "corrupt-damage", damaged });
    await runPhase("corrupt", page, { expect: "the store fails its check and the node bootstraps", waitCheckpoint: true });
  }
  const quotaMb = Number(env("QUOTA_MB", 0));
  if (quotaMb > 0) {
    await page.close();
    page = await newPage();
    phase = "quota";
    phaseStart = Date.now();
    await page.goto(base + "/build-info.json", { waitUntil: "domcontentloaded" });
    await sleep(2000);
    await page.evaluate(async () => {
      const root = await navigator.storage.getDirectory();
      await root.removeEntry("mina-frontier", { recursive: true }).catch(() => {});
    });
    const cdp = await page.createCDPSession();
    await cdp.send("Storage.overrideQuotaForOrigin", { origin: new URL(base).origin, quotaSize: quotaMb * 2 ** 20 });
    log(`quota: the origin's quota set to ${quotaMb} MiB`);
    const quotaLine = /QuotaExceeded|quota/i;
    const summary = await runPhase("quota", page, { expect: "the store turned off by the quota, the node synced", waitCheckpoint: false });
    const turnedOff = await until(page, () => consoleLines.some((l) => l.phase === "quota" && !l.tab && quotaLine.test(l.text) && /Error|Disabled|disable/.test(l.text)), "the store turned off", { limit: checkpointWait });
    jsonl("phases.jsonl", {
      phase: "quota-result",
      turned_off_s: turnedOff.at,
      synced_at_tip_s: summary.synced_at_tip_s,
      still_synced: turnedOff.last?.status?.sync ?? null,
      lines: consoleLines.filter((l) => l.phase === "quota" && !l.tab && quotaLine.test(l.text)).map((l) => l.text.slice(0, 400)),
    });
    log(`quota: turned off by ${turnedOff.at} s, sync ${turnedOff.last?.status?.sync}`);
  }
  await page.close();
} catch (error) {
  log(`error: ${error.stack ?? error.message}`);
  exit = 1;
} finally {
  await browser.close();
  rmSync(join(out, ENDPOINT_FILE), { force: true });
  rmSync(profile, { recursive: true, force: true });
  release(lock);
  log(`done in ${Math.round((Date.now() - started) / 1000)} s`);
}
process.exit(exit);
