// Attach to the browser started by run.mjs, sign inside the page, and inject
// through the WASM node. The decrypted throwaway key travels only through
// anonymous pipe fd 3 and one DevTools evaluate argument. Run from a private
// .work directory; do not redirect DevTools traffic or enable protocol logs.
//
// Needs RUN_DIR, P416_DECRYPT, P416_KEY_FILE, P416_PASS_FILE, and
// P416_RELAY_GRAPHQL. The served page must have /p416/mina-signer.js copied
// from frontend/node_modules/mina-signer/dist/web/index.js.
import { spawnSync } from "node:child_process";
import { attach, loadPuppeteer, webnodePage } from "./lib.mjs";

const sender = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
const recipient = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
const relay = process.env.P416_RELAY_GRAPHQL ?? "http://127.0.0.1:3540/graphql";
const required = ["P416_DECRYPT", "P416_KEY_FILE", "P416_PASS_FILE", "RUN_DIR"];
for (const name of required) {
  if (!process.env[name]) throw Error(`${name} is required`);
}

async function graphql(query) {
  const response = await fetch(relay, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
    signal: AbortSignal.timeout(10000),
  });
  const json = await response.json();
  if (!response.ok || json.errors?.length)
    throw Error("relay GraphQL query failed");
  return json.data;
}

const before = await graphql(
  `query { account(publicKey: "${sender}") { nonce balance { total } } syncStatus }`,
);
if (before.syncStatus !== "SYNCED" || !before.account?.nonce) {
  throw Error("relay is not synced or sender account is missing");
}
const nonce = Number(before.account.nonce);

const browser = await attach(loadPuppeteer());
const page = await webnodePage(browser);
const status = await page.evaluate(() => window.webnode?.status());
if (status?.transition_frontier?.sync?.status !== "Synced") {
  throw Error("browser web node is not synced");
}

const decrypted = spawnSync(
  process.env.P416_DECRYPT,
  [process.env.P416_KEY_FILE, process.env.P416_PASS_FILE],
  { stdio: ["ignore", "pipe", "pipe", "pipe"], encoding: "utf8" },
);
if (decrypted.status !== 0) {
  throw Error("throwaway key decryption failed");
}
const secret = Buffer.from(decrypted.output[3] ?? "", "utf8");
if (!/^EK[A-Za-z0-9]{50,60}$/.test(secret.toString())) {
  secret.fill(0);
  throw Error("decrypted throwaway key has an unexpected encoding");
}

let signed;
try {
  signed = await page.evaluate(
    async (privateKey, sender, recipient, nonce) => {
      try {
        const { default: Client } = await import("/p416/mina-signer.js");
        const client = new Client({ network: "testnet" });
        const payment = {
          from: sender,
          to: recipient,
          amount: "1000000000",
          fee: "100000000",
          nonce: String(nonce),
          validUntil: "4294967295",
          memo: "mina-rust web node P4.16",
        };
        const signedPayment = client.signPayment(payment, privateKey);
        if (!client.verifyPayment(signedPayment)) {
          throw Error("browser payment signature failed local verification");
        }
        const hash = client.hashPayment(signedPayment, { berkeley: true });
        const injected = await window.webnode
          .transaction_pool()
          .inject()
          .payment({
            signature_field: signedPayment.signature.field,
            signature_scalar: signedPayment.signature.scalar,
            valid_until: Number(payment.validUntil),
            amount: Number(payment.amount),
            fee: Number(payment.fee),
            from: payment.from,
            to: payment.to,
            nonce: Number(payment.nonce),
            memo: payment.memo,
          });
        return { hash, injected };
      } finally {
        privateKey = null;
      }
    },
    secret.toString(),
    sender,
    recipient,
    nonce,
  );
} finally {
  secret.fill(0);
}

const submittedAt = new Date().toISOString();
// RpcTransactionInjectResponse is serde(untagged): Success is an array of
// validated command objects, Rejected an array of pairs, Failure strings.
const accepted =
  Array.isArray(signed?.injected) &&
  signed.injected.some(
    item => item && typeof item === "object" && !Array.isArray(item),
  );
if (!signed?.hash || !accepted) {
  await page.goto("about:blank").catch(() => {});
  browser.disconnect();
  throw Error("browser web node did not accept the payment");
}
let poolSeenAt = null;
for (let i = 0; i < 70; i++) {
  const data = await graphql("query { pooledUserCommands { hash } }");
  if (data.pooledUserCommands?.some(tx => tx.hash === signed.hash)) {
    poolSeenAt = new Date().toISOString();
    break;
  }
  await new Promise(resolve => setTimeout(resolve, 2000));
}

// Navigation destroys the document and worker after relay receipt, clearing
// the page's transient key argument. The run.mjs browser is then stopped by PID.
await page.goto("about:blank").catch(() => {});
browser.disconnect();
console.log(
  JSON.stringify({
    sender,
    recipient,
    nonce,
    amountNanomina: 1000000000,
    feeNanomina: 100000000,
    memo: "mina-rust web node P4.16",
    hash: signed.hash,
    submittedAt,
    poolSeenAt,
    relay: poolSeenAt ? "seen" : "not seen within 140 seconds",
    injection: "Success",
    signing: "mina-signer in browser",
    broadcast: "webnode.transaction_pool().inject().payment",
    keyCleared: "page navigated to about:blank after submission",
    before,
  }),
);
