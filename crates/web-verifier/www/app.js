import init, { initThreadPool, createVerifier } from "./pkg/mina_web_verifier.js";

const BUCKET_URL = "https://storage.googleapis.com/storage/v1/b/mina_network_block_data/o";

// Heights are six digits for now; the glob keeps old short-height names out of
// lexicographic range queries.
const NETWORKS = {
  devnet: { forkHeight: 545433, vk: "vk/devnet-mesa-blockchain-vk.json" },
  mainnet: { forkHeight: 548146, vk: "vk/mainnet-mesa-blockchain-vk.json" },
};
const HEIGHT_GLOB = "[0-9][0-9][0-9][0-9][0-9][0-9]";
// A Mesa slot is 90 s; a tip older than this many slots is stale.
const STALE_AFTER_MS = 6 * 90_000;

const $ = (id) => document.getElementById(id);
const verifiers = new Map();
let threadPool = null;
let lastVerified = null;
let socket = null;

// Every verified block of the current run, by state hash, for chain labels.
const seen = new Map();
let bestTip = null;

function seconds(ms) {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

function ago(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 120 ? `${s} s ago` : `${Math.round(s / 60)} min ago`;
}

function step(label) {
  const item = document.createElement("li");
  item.textContent = `${label}…`;
  $("setup").append(item);
  const started = performance.now();
  return {
    done(detail = "") {
      item.textContent = `${label} `;
      const time = document.createElement("span");
      time.className = "time";
      time.textContent = `(${seconds(performance.now() - started)})`;
      item.append(time);
      if (detail) item.append(` ${detail}`);
    },
    fail(error) {
      item.textContent = `${label}: `;
      const message = document.createElement("span");
      message.className = "bad";
      message.textContent = String(error);
      item.append(message);
    },
  };
}

async function timed(label, work, detail) {
  const s = step(label);
  try {
    const value = await work();
    s.done(typeof detail === "function" ? detail(value) : detail);
    return value;
  } catch (error) {
    s.fail(error);
    throw error;
  }
}

async function sha256(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function startThreads() {
  threadPool ??= (async () => {
    if (!crossOriginIsolated) {
      throw new Error("page is not cross-origin isolated, so wasm threads are unavailable");
    }
    await timed("Load WebAssembly verifier", () => init());
    const threads = Math.max(1, (navigator.hardwareConcurrency || 4) - 1);
    await timed("Start thread pool", () => initThreadPool(threads), `${threads} workers`);
  })();
  return threadPool;
}

function verifierFor(network) {
  if (!verifiers.has(network)) {
    verifiers.set(network, (async () => {
      await startThreads();
      const vkJson = await timed(`Fetch ${network} verification key`, async () => {
        const res = await fetch(NETWORKS[network].vk);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.text();
      });
      const digest = await sha256(vkJson);
      const verifier = await timed(
        `Build ${network} verifier index and SRS`,
        () => createVerifier(vkJson),
        `key sha256 ${digest.slice(0, 16)}…`,
      );
      await timed("Build proof-checking SRS", () => verifier.warmUp());
      return verifier;
    })());
    verifiers.get(network).catch(() => verifiers.delete(network));
  }
  return verifiers.get(network);
}

// Bucket source: the public archive of precomputed blocks.

async function listBlocks(network, fromHeight, maxResults) {
  const params = new URLSearchParams({
    matchGlob: `${network}-${HEIGHT_GLOB}-*.json`,
    startOffset: `${network}-${fromHeight}`,
    endOffset: `${network}-a`,
    maxResults: String(maxResults),
    fields: "items(name)",
  });
  const res = await fetch(`${BUCKET_URL}?${params}`);
  if (!res.ok) throw new Error(`bucket listing failed: HTTP ${res.status}`);
  const body = await res.json();
  return (body.items ?? []).flatMap(({ name }) => {
    const match = /^[a-z]+-(\d{6})-(3N[1-9A-HJ-NP-Za-km-z]+)\.json$/.exec(name);
    return match ? [{ name, height: Number(match[1]) }] : [];
  });
}

// The highest height with at least one block in the bucket, by binary search.
async function newestHeight(network) {
  let low = NETWORKS[network].forkHeight;
  let high = 1_000_000;
  while (high - low > 1) {
    const mid = Math.floor((low + high) / 2);
    const found = await listBlocks(network, mid, 1);
    if (found.length > 0) low = mid;
    else high = mid;
  }
  return low;
}

async function fetchBlock(name) {
  const res = await fetch(`${BUCKET_URL}/${encodeURIComponent(name)}?alt=media`);
  if (!res.ok) throw new Error(`block download failed: HTTP ${res.status}`);
  return res.text();
}

// Results table and chain labels.

function resetRun() {
  $("blocks").tBodies[0].replaceChildren();
  $("summary").textContent = "";
  seen.clear();
  bestTip = null;
  renderTip();
}

function addRow(label) {
  const row = document.createElement("tr");
  for (let i = 0; i < 7; i++) row.append(document.createElement("td"));
  row.cells[0].textContent = label;
  row.cells[3].className = "hash";
  row.cells[4].className = "num";
  row.cells[5].className = "pending";
  row.cells[5].textContent = "verifying…";
  $("blocks").tBodies[0].prepend(row);
  return row;
}

function badge(cell, text, className = "fork") {
  const span = document.createElement("span");
  span.className = className;
  span.textContent = text;
  cell.append(span);
}

// Label each verified block by its relation to the best tip: walk parent links
// from the tip through the blocks seen so far.
function relabel() {
  const canonical = new Set();
  for (let hash = bestTip?.state_hash; hash && seen.has(hash); hash = seen.get(hash).facts.previous_state_hash) {
    canonical.add(hash);
  }
  const perHeight = new Map();
  for (const { facts } of seen.values()) perHeight.set(facts.height, (perHeight.get(facts.height) ?? 0) + 1);

  for (const [hash, { facts, row }] of seen) {
    const cell = row.cells[6];
    cell.replaceChildren();
    if (hash === bestTip?.state_hash) {
      cell.className = "ok";
      cell.textContent = "best tip";
    } else if (canonical.has(hash)) {
      cell.className = "";
      cell.textContent = `canonical, ${bestTip.height - facts.height} deep`;
    } else if (facts.height <= bestTip.height) {
      cell.className = "muted";
      cell.textContent = "not on best chain";
    } else {
      cell.className = "muted";
      cell.textContent = "";
    }
    if (perHeight.get(facts.height) > 1 && !row.cells[0].querySelector(".fork")) {
      badge(row.cells[0], "competing");
    }
  }
}

function renderTip() {
  if (!bestTip) {
    $("tip").textContent = "No verified block yet.";
    return;
  }
  const age = Date.now() - bestTip.timestamp_ms;
  $("tip").replaceChildren();
  const parts = [
    ["Height", String(bestTip.height)],
    ["State hash", bestTip.state_hash],
    ["Produced", ago(age)],
    ["Min window density", String(bestTip.min_window_density)],
    ["Verified blocks", String(seen.size)],
  ];
  for (const [label, value] of parts) {
    const item = document.createElement("div");
    const name = document.createElement("span");
    name.className = "muted";
    name.textContent = label;
    const text = document.createElement("strong");
    text.textContent = value;
    item.append(name, text);
    $("tip").append(item);
  }
  if (age > STALE_AFTER_MS) {
    const warning = document.createElement("div");
    warning.className = "bad";
    warning.textContent = "Tip is stale: the source may be withholding blocks.";
    $("tip").append(warning);
  }
}

async function verifyInto(row, verify) {
  const started = performance.now();
  try {
    const facts = JSON.parse(await verify());
    const elapsed = performance.now() - started;
    row.cells[0].textContent = String(facts.height);
    row.cells[1].textContent = String(facts.global_slot_since_genesis);
    row.cells[2].textContent = new Date(facts.timestamp_ms).toISOString().replace("T", " ").slice(0, 19);
    row.cells[3].textContent = `${facts.state_hash.slice(0, 14)}…${facts.state_hash.slice(-6)}`;
    row.cells[3].title = facts.state_hash;
    row.cells[4].textContent = seconds(elapsed);
    row.cells[5].className = "ok";
    row.cells[5].textContent = "✓ valid";
    seen.set(facts.state_hash, { facts, row });
    if (facts.state_hash === facts.best_tip_state_hash) bestTip = facts;
    relabel();
    renderTip();
    return { ok: true, elapsed, facts };
  } catch (error) {
    row.cells[4].textContent = seconds(performance.now() - started);
    row.cells[5].className = "bad";
    row.cells[5].textContent = `✗ ${error}`;
    return { ok: false };
  }
}

function summarize(results, source) {
  const valid = results.filter((r) => r.ok);
  const mean = valid.reduce((sum, r) => sum + r.elapsed, 0) / Math.max(valid.length, 1);
  $("summary").textContent =
    `${valid.length} of ${results.length} block proofs from ${source} verified in this browser` +
    (valid.length ? `, ${seconds(mean)} each on average.` : ".");
}

async function verifyBucket() {
  const network = $("network").value;
  const count = Math.min(50, Math.max(1, Number($("count").value) || 8));
  const verifier = await verifierFor(network);
  const newest = await timed(`Find newest ${network} block in bucket`, () => newestHeight(network), (h) => `height ${h}`);
  const lowest = Math.max(newest - count + 1, NETWORKS[network].forkHeight + 1);
  // Every block at the chosen heights, competing ones included, oldest first.
  const blocks = (await listBlocks(network, lowest, 500))
    .filter((b) => b.height >= lowest && b.height <= newest)
    .sort((a, b) => a.height - b.height);

  const results = [];
  for (const block of blocks) {
    const row = addRow(String(block.height));
    const json = await fetchBlock(block.name);
    results.push(await verifyInto(row, () => verifier.verifyPrecomputed(json)));
    lastVerified = { network, json };
    $("tamper").disabled = false;
  }
  summarize(results, "the bucket");
}

// Live source: blocks as they are gossiped, forwarded by a relay.

async function relayStatus(wsUrl) {
  const url = new URL(wsUrl);
  url.protocol = url.protocol === "wss:" ? "https:" : "http:";
  url.pathname = "/status";
  const res = await fetch(url, { mode: "cors" });
  if (!res.ok) throw new Error(`relay status: HTTP ${res.status}`);
  return res.json();
}

async function verifyLive() {
  const url = $("relay").value.trim();
  const status = await timed("Contact relay", () => relayStatus(url), (s) => `${s.network}, ${s.peers} peers`);
  if (!NETWORKS[status.network]) throw new Error(`relay serves unknown network ${status.network}`);
  $("network").value = status.network;
  const verifier = await verifierFor(status.network);

  const results = [];
  let queue = Promise.resolve();
  await new Promise((resolve, reject) => {
    socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      step("Subscribed to gossiped blocks").done(`network ${status.network}`);
      $("summary").textContent = "Waiting for blocks; Mesa produces one every few minutes.";
      resolve();
    };
    socket.onerror = () => reject(new Error(`could not connect to ${url}`));
    socket.onclose = () => {
      $("verify").textContent = "Start";
      socket = null;
    };
    socket.onmessage = ({ data }) => {
      const payload = new Uint8Array(data);
      queue = queue.then(async () => {
        const row = addRow("…");
        const result = await verifyInto(row, () => verifier.verifyGossipBlock(payload));
        results.push(result);
        if (result.ok) {
          lastVerified = { network: status.network, json: null };
        }
        summarize(results, "live gossip");
      });
    };
  });
  $("verify").textContent = "Stop";
}

async function start() {
  if (socket) {
    socket.close();
    return;
  }
  $("verify").disabled = true;
  $("tamper").disabled = true;
  resetRun();
  try {
    if ($("source").value === "live") await verifyLive();
    else await verifyBucket();
  } catch (error) {
    $("summary").textContent = `Stopped: ${error}`;
  } finally {
    $("verify").disabled = false;
  }
}

async function tamper() {
  if (!lastVerified?.json) return;
  const block = JSON.parse(lastVerified.json);
  const state = block.data?.protocol_state ?? block.protocol_state;
  const length = state.body.consensus_state.blockchain_length;
  state.body.consensus_state.blockchain_length = String(Number(length) + 1);
  const row = addRow(`${length} → ${Number(length) + 1}`);
  badge(row.cells[0], "tampered");
  const verifier = await verifierFor(lastVerified.network);
  await verifyInto(row, () => verifier.verifyPrecomputed(JSON.stringify(block)));
}

function showSourceControls() {
  const live = $("source").value === "live";
  for (const el of document.querySelectorAll("[data-source]")) {
    el.hidden = el.dataset.source !== (live ? "live" : "bucket");
  }
  $("verify").textContent = live ? "Start" : "Verify latest blocks";
}

$("source").addEventListener("change", showSourceControls);
$("verify").addEventListener("click", start);
$("tamper").addEventListener("click", tamper);
setInterval(renderTip, 1000);
showSourceControls();
