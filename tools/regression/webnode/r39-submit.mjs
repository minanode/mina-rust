// Attach ONLY to run.mjs's RUN_DIR. Decrypt the authorised throwaway key into
// anonymous pipe fd 3, then pass one transient CDP argument to Rust/WASM.
// No JS signer, GraphQL mutation, key files, storage, console or screenshots.
// JS/CDP strings and signer allocations cannot be reliably zeroed.
// MODE=payment|zkapp, R39_DECRYPT, R39_KEY_FILE, R39_PASS_FILE, RUN_DIR,
// R39_RELAY_GRAPHQL (our isolated relay), and RECEIPT (public output) required.
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { browserEndpoint, loadPuppeteer, webnodePage } from "./lib.mjs";

const sender = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
const recipient = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
for (const name of [
  "MODE",
  "R39_DECRYPT",
  "R39_KEY_FILE",
  "R39_PASS_FILE",
  "RUN_DIR",
  "R39_RELAY_GRAPHQL",
  "RECEIPT",
]) {
  if (!process.env[name]) throw Error(`${name} is required`);
}
const mode = process.env.MODE;
if (!["payment", "zkapp"].includes(mode))
  throw Error("MODE must be payment or zkapp");
const endpoints = {
  relay: process.env.R39_RELAY_GRAPHQL,
  devnetRelay: "http://127.0.0.1:3540/graphql",
};
async function query(url, query) {
  if (!query.startsWith("query ")) throw Error("read-only GraphQL required");
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
    signal: AbortSignal.timeout(15000),
  });
  const data = await response.json();
  if (!response.ok || data.errors?.length)
    throw Error("read-only GraphQL failed");
  return data.data;
}
const before = {};
for (const [name, url] of Object.entries(endpoints)) {
  before[name] = await query(
    url,
    `query { syncStatus account(publicKey: "${sender}") { nonce balance { total } } bestChain(maxLength: 1) { protocolState { consensusState { blockHeight } } } }`,
  );
  if (before[name].syncStatus !== "SYNCED")
    throw Error(`${name} is not synced`);
}
const nonce = Number(before.relay.account.nonce);
if (nonce !== Number(before.devnetRelay.account.nonce))
  throw Error("relay nonces disagree");
const phase = name =>
  console.log(JSON.stringify({ phase: name, at: new Date().toISOString() }));
phase("attach");
// Do not configure the Network domain on every threaded WASM worker. Chrome
// can leave those commands unanswered on a second DevTools attachment.
const browser = await loadPuppeteer().connect({
  browserWSEndpoint: browserEndpoint(),
  protocolTimeout: 120000,
  networkEnabled: false,
});
let secret, page;
try {
  page = await webnodePage(browser);
  phase("status");
  const status = await page.evaluate(() => window.webnode?.status());
  const tipHeight = Number(status?.transition_frontier?.best_tip?.height);
  const relayHeight = Number(
    before.relay.bestChain[0].protocolState.consensusState.blockHeight,
  );
  if (
    status?.transition_frontier?.sync?.status !== "Synced" ||
    !tipHeight ||
    tipHeight < relayHeight - 2
  )
    throw Error("browser is not synced to the live devnet tip");

  phase("http-guard");
  await page.evaluate(() => {
    const requests = [];
    const oldFetch = window.fetch;
    const oldOpen = XMLHttpRequest.prototype.open;
    const record = (url, method) => {
      const u = new URL(url, location.href);
      const blocked = /graphql|send-payment/i.test(u.pathname);
      requests.push({
        at: new Date().toISOString(),
        method,
        origin: u.origin,
        path: u.pathname,
        blocked,
      });
      if (blocked) throw Error("R39 blocks page HTTP transaction submission");
    };
    window.fetch = function (input, init) {
      record(
        input instanceof Request ? input.url : input,
        init?.method ?? (input instanceof Request ? input.method : "GET"),
      );
      return oldFetch.call(this, input, init);
    };
    XMLHttpRequest.prototype.open = function (method, url, ...rest) {
      record(url, method);
      return oldOpen.call(this, method, url, ...rest);
    };
    // This holds only public request metadata and restore functions, no key.
    window.__r39HttpAudit = {
      requests,
      restore() {
        window.fetch = oldFetch;
        XMLHttpRequest.prototype.open = oldOpen;
        delete window.__r39HttpAudit;
      },
    };
  });
  // No encoding option: output[3] is the original mutable Buffer, without
  // spawnSync retaining a second immutable plaintext output string.
  phase("decrypt-to-pipe");
  const decrypted = spawnSync(
    process.env.R39_DECRYPT,
    [process.env.R39_KEY_FILE, process.env.R39_PASS_FILE],
    { stdio: ["ignore", "ignore", "ignore", "pipe"] },
  );
  secret = decrypted.output?.[3];
  if (
    decrypted.status !== 0 ||
    !Buffer.isBuffer(secret) ||
    secret.length < 50 ||
    secret.length > 60
  )
    throw Error("throwaway key pipe failed");
  const startedAt = new Date().toISOString();
  phase("wasm-sign-and-inject");
  const signed = await page.evaluate(
    async (privateKey, transfer, mode) => {
      try {
        const injector = window.webnode.transaction_pool().inject();
        return mode === "payment"
          ? await injector.sign_and_inject_payment(privateKey, transfer)
          : await injector.sign_and_inject_zkapp_transfer(privateKey, transfer);
      } finally {
        privateKey = null;
      }
    },
    secret.toString("utf8"),
    {
      from: sender,
      to: recipient,
      amount: "1000000000",
      fee: "100000000",
      nonce,
      memo: "mina-rust web node R39",
      valid_until: null,
    },
    mode,
  );
  secret.fill(0);
  const receipt = {
    mode,
    sender,
    recipient,
    nonce,
    amount: "1000000000",
    fee: "100000000",
    memo: "mina-rust web node R39",
    startedAt,
    responseAt: new Date().toISOString(),
    signed,
    before,
    status,
    keyHandling:
      "anonymous pipe Buffer overwritten; JS/CDP/signer copies cannot be reliably erased",
    pools: {},
  };
  writeFileSync(process.env.RECEIPT, JSON.stringify(receipt, null, 2));
  if (
    signed?.accepted !== true ||
    signed.signing !== "rust-wasm" ||
    signed.network !== "devnet"
  )
    throw Error(
      "Rust/WASM injection was not accepted; inspect public receipt, do not resubmit blindly",
    );
  const field =
    mode === "payment" ? "pooledUserCommands" : "pooledZkappCommands";
  for (let i = 0; i < 100 && Object.keys(receipt.pools).length < 2; i++) {
    for (const [name, url] of Object.entries(endpoints)) {
      if (receipt.pools[name]) continue;
      const data = await query(url, `query { ${field} { hash } }`);
      if (data[field]?.some(t => t.hash === signed.hash))
        receipt.pools[name] = new Date().toISOString();
    }
    writeFileSync(process.env.RECEIPT, JSON.stringify(receipt, null, 2));
    if (Object.keys(receipt.pools).length < 2)
      await new Promise(r => setTimeout(r, 1500));
  }
  const requests = await page.evaluate(() => window.__r39HttpAudit.requests);
  receipt.browserHttp = requests;
  writeFileSync(
    process.env.RECEIPT + ".http.jsonl",
    requests.map(r => JSON.stringify(r) + "\n").join(""),
  );
  writeFileSync(process.env.RECEIPT, JSON.stringify(receipt, null, 2));
  console.log(
    JSON.stringify({
      mode,
      hash: signed.hash,
      accepted: signed.accepted,
      pools: receipt.pools,
      browserGraphqlAttempts: requests.filter(r => r.blocked).length,
    }),
  );
} finally {
  secret?.fill(0);
  if (page)
    await page.evaluate(() => window.__r39HttpAudit?.restore()).catch(() => {});
  // Keep the node alive for the second command and transport evidence. The
  // owner stops run.mjs by PID; this disconnection releases only our CDP client.
  browser.disconnect();
}
