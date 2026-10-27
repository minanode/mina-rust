// W2: mainnet web-node memory soak in a remote browser slot.
// Set CHROME, BASE, RELAY_HTTP, OUT and PUPPETEER_DIR. The remote slot owns the
// browser lock. Samples every minute for 12 hours after the first Synced status.
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { findPuppeteer } from "./lib.mjs";

const { puppeteer } = findPuppeteer();
const out = process.env.OUT;
const base = process.env.BASE;
const relay = process.env.RELAY_HTTP;
if (!out || !base || !relay || !process.env.CHROME)
  throw new Error("set OUT, BASE, RELAY_HTTP, CHROME");
mkdirSync(out, { recursive: true });
const logPath = join(out, "console.log");
writeFileSync(logPath, "");
const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu"],
  defaultViewport: { width: 1600, height: 1000 },
  protocolTimeout: 90000,
});
const browserCdp = await browser.target().createCDPSession();
const started = Date.now();
const lines = join(out, "measurements.jsonl");
writeFileSync(lines, "");
const page = await browser.newPage();
page.on("console", m =>
  appendFileSync(
    logPath,
    `${new Date().toISOString()} [${m.type()}] ${m.text()}\n`,
  ),
);
page.on("pageerror", e =>
  appendFileSync(
    logPath,
    `${new Date().toISOString()} [pageerror] ${e.stack ?? e}\n`,
  ),
);
page.on("requestfailed", r =>
  appendFileSync(
    logPath,
    `${new Date().toISOString()} [requestfailed] ${r.url()} ${r.failure()?.errorText}\n`,
  ),
);

// evaluateOnNewDocument also runs in same-origin child frames. Capture the
// memory object before wasm-bindgen starts, since the exported instance hides it.
await page.evaluateOnNewDocument(() => {
  const Original = WebAssembly.Memory;
  WebAssembly.Memory = function (desc) {
    const memory = new Original(desc);
    (globalThis.__w2Memories ||= []).push(memory);
    return memory;
  };
  WebAssembly.Memory.prototype = Original.prototype;
});

async function processTreeMemory(rootPid) {
  let cdpRoles = new Map();
  try {
    const result = await browserCdp.send("SystemInfo.getProcessInfo");
    cdpRoles = new Map(result.processInfo.map(p => [p.id, p.type]));
  } catch (error) {
    appendFileSync(logPath, `${new Date().toISOString()} [cdp] ${error}\n`);
  }
  const rows = execFileSync("ps", ["-e", "-o", "pid=,ppid=,rss="], {
    encoding: "utf8",
  })
    .trim()
    .split("\n")
    .map(s => s.trim().split(/\s+/).map(Number));
  const children = new Map();
  for (const [pid, parent, rss] of rows) {
    if (!children.has(parent)) children.set(parent, []);
    children.get(parent).push([pid, rss]);
  }
  const stack = [[rootPid, rows.find(r => r[0] === rootPid)?.[2] ?? 0]];
  const seen = new Set();
  const processes = [];
  while (stack.length) {
    const [pid, rss] = stack.pop();
    if (seen.has(pid)) continue;
    seen.add(pid);
    let pssKiB = null;
    try {
      const rollup = readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
      pssKiB = Number(rollup.match(/^Pss:\s+(\d+) kB/m)?.[1] ?? NaN);
      if (!Number.isFinite(pssKiB)) pssKiB = null;
    } catch {}
    let role = cdpRoles.get(pid) ?? (pid === rootPid ? "browser" : "other");
    try {
      const args = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
      const type = args.find(a => a.startsWith("--type="))?.slice(7);
      const utility = args
        .find(a => a.startsWith("--utility-sub-type="))
        ?.slice(19);
      if (type && !cdpRoles.has(pid))
        role =
          type === "utility" && utility?.includes("NetworkService")
            ? "utility/network"
            : type;
    } catch {}
    processes.push({ pid, role, rss_kib: rss, pss_kib: pssKiB });
    stack.push(...(children.get(pid) ?? []));
  }
  return {
    rss_mib: processes.reduce((a, p) => a + p.rss_kib, 0) / 1024,
    pss_mib: processes.every(p => p.pss_kib !== null)
      ? processes.reduce((a, p) => a + p.pss_kib, 0) / 1024
      : null,
    processes,
  };
}

async function getJson(url) {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } catch (e) {
    return { error: String(e) };
  }
}

async function sample() {
  const now = Date.now();
  const status = await page
    .evaluate(async () => {
      if (!window.webnode?.status) return null;
      return await Promise.race([
        window.webnode.status().then(s => ({
          sync: s?.transition_frontier?.sync?.status ?? null,
          height: s?.transition_frontier?.best_tip?.height ?? null,
          peers:
            s?.peers?.filter(p => p.connection_status === "Connected").length ??
            null,
        })),
        new Promise(resolve =>
          setTimeout(() => resolve({ error: "status timeout" }), 10000),
        ),
      ]);
    })
    .catch(e => ({ error: String(e) }));
  const frames = await Promise.all(
    page.frames().map(async frame => {
      try {
        return await frame.evaluate(() => ({
          realm: globalThis.__MINA_BLOCK_PROVER_REALM__
            ? "prover"
            : window === top
              ? "node"
              : "other-frame",
          wasm_bytes: (globalThis.__w2Memories ?? []).map(
            m => m.buffer.byteLength,
          ),
          js_heap_bytes: performance.memory?.usedJSHeapSize ?? null,
        }));
      } catch (e) {
        return { error: String(e) };
      }
    }),
  );
  const relayStatus = await getJson(`${relay}/status`);
  const tree = await processTreeMemory(browser.process().pid);
  const row = {
    unix: now / 1000,
    elapsed_seconds: (now - started) / 1000,
    browser_rss_mib: tree.rss_mib,
    browser_pss_mib: tree.pss_mib,
    processes: tree.processes,
    frames,
    status,
    relay: {
      sync: relayStatus?.transition_frontier?.sync?.status ?? null,
      height: relayStatus?.transition_frontier?.best_tip?.height ?? null,
      peers:
        relayStatus?.peers?.filter(p => p.connection_status === "Connected")
          .length ?? null,
      error: relayStatus.error ?? null,
    },
  };
  appendFileSync(lines, JSON.stringify(row) + "\n");
  console.log(
    JSON.stringify({
      unix: row.unix,
      sync: status?.sync,
      height: status?.height,
      relay_height: row.relay.height,
      wasm: frames.map(f => f.wasm_bytes),
      rss_mib: tree.rss_mib,
    }),
  );
  return row;
}

try {
  await page.goto(`${base}/w2-empty`, { waitUntil: "domcontentloaded" });
  if (!(await page.evaluate(() => crossOriginIsolated)))
    throw new Error("baseline COOP/COEP isolation absent");
  const baseline = [];
  baseline.push({
    unix: Date.now() / 1000,
    ...(await processTreeMemory(browser.process().pid)),
  });
  await new Promise(resolve => setTimeout(resolve, 60000));
  baseline.push({
    unix: Date.now() / 1000,
    ...(await processTreeMemory(browser.process().pid)),
  });
  writeFileSync(
    join(out, "baseline.json"),
    JSON.stringify(baseline, null, 2) + "\n",
  );
  await page.goto(`${base}/loading-web-node`, {
    waitUntil: "domcontentloaded",
    timeout: 120000,
  });
  if (!(await page.evaluate(() => crossOriginIsolated)))
    throw new Error("COOP/COEP isolation absent");
  await page.waitForFunction(
    () =>
      [...document.querySelectorAll("button")].some(b =>
        ["Start the web node", "Start Non BP"].includes(b.innerText.trim()),
      ),
    { timeout: 60000 },
  );
  await page.evaluate(() =>
    [...document.querySelectorAll("button")]
      .find(b =>
        ["Start the web node", "Start Non BP"].includes(b.innerText.trim()),
      )
      .click(),
  );
  let syncedAt = null;
  while (Date.now() - started < 14 * 3600 * 1000) {
    const row = await sample();
    const webHeight = Number(row.status?.height);
    const relayHeight = Number(row.relay.height);
    if (
      row.status?.sync === "Synced" &&
      row.status?.height != null &&
      row.relay.height != null &&
      Number.isFinite(webHeight) &&
      Number.isFinite(relayHeight) &&
      webHeight >= relayHeight - 1 &&
      syncedAt === null
    ) {
      syncedAt = Date.now();
      writeFileSync(
        join(out, "synced.json"),
        JSON.stringify({ unix: syncedAt / 1000, row }) + "\n",
      );
    }
    if (syncedAt && Date.now() - syncedAt >= 12 * 3600 * 1000) break;
    await new Promise(resolve => setTimeout(resolve, syncedAt ? 60000 : 10000));
  }
  await page.screenshot({ path: join(out, "final.png") });
} finally {
  await browser.close();
}
