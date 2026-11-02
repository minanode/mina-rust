// MINAWEB-KEYFILE-IMPORT: drive the web node page's key file import and
// export (ui/minaweb, /key) in headless Chrome, with throwaway keys, and
// check that the raw key never leaves the tab.
//
//   1. Make a throwaway key here (a random scalar, in this process's memory
//      only), and have the native node write it as an encrypted key file
//      (`mina wallet generate`, MINA_SEC_KEY and MINA_PRIVKEY_PASS in its
//      environment only) in KEY_DIR, a 0700 directory.
//   2. Start the node, open the key screen, and try the key file import:
//      a wrong password, then the right one. The page's public key must be
//      the file's. Time the decryption, and the page thread's longest stall
//      while it runs.
//   3. Export the key as a key file under a new password; the native node
//      must read the download back to the same public key.
//   4. Forget the key, and import OCAML_FILE (a key file the OCaml node
//      wrote) with OCAML_PASSWORD: the page's public key must be its .pub.
//
// The key-leak probe (key-leak-probe.mjs) watches the document, its frames
// and every worker from before the first key exists, with the raw key and
// both passwords as its needles; a positive control shows it sees what it
// watches. Storage, IndexedDB names, requests and console lines are checked
// too. Any finding, or any Content-Security-Policy violation, fails the run.
//
// Environment: OUT, BASE, CHROME, PROFILE_DIR, KEY_DIR, MINA_BIN (a native
// `mina`), OCAML_FILE and OCAML_PASSWORD (optional), CSP_REPORTS as
// csp-violations.mjs, PUPPETEER_DIR as lib.mjs.
import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

import {
  reportedViolations,
  servedPolicy,
  watchCsp,
} from "./csp-violations.mjs";
import {
  PROBE,
  exerciseProbe,
  probePassed,
  watchWorkers,
} from "./key-leak-probe.mjs";
import { findPuppeteer } from "./lib.mjs";

const { puppeteer } = findPuppeteer();
const env = process.env;
for (const k of ["OUT", "BASE", "CHROME", "PROFILE_DIR", "KEY_DIR", "MINA_BIN"])
  if (!env[k]) throw new Error(`set ${k}`);
const out = env.OUT;
mkdirSync(out, { recursive: true });
mkdirSync(env.PROFILE_DIR, { recursive: true, mode: 0o700 });
mkdirSync(env.KEY_DIR, { recursive: true, mode: 0o700 });
const summary = { started: new Date().toISOString(), shots: [], errors: [] };
const save = () =>
  writeFileSync(join(out, "summary.json"), JSON.stringify(summary, null, 2));
const sleep = ms => new Promise(r => setTimeout(r, ms));
const consoleFile = join(out, "console.log");

// 1. A throwaway key: base58check of version 90, 1, a 32-byte scalar. Its
// top two bits are clear at both ends, below the Pallas scalar field's
// order whichever byte order the encoding uses.
const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
function base58(bytes) {
  let n = BigInt(`0x${Buffer.from(bytes).toString("hex")}`);
  let text = "";
  while (n > 0n) {
    text = ALPHABET[Number(n % 58n)] + text;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    text = `1${text}`;
  }
  return text;
}
const sha = b => createHash("sha256").update(b).digest();
const scalar = randomBytes(32);
scalar[0] &= 0x3f;
scalar[31] &= 0x3f;
const payload = Buffer.concat([Buffer.from([90, 1]), scalar]);
const privateKey = base58(
  Buffer.concat([payload, sha(sha(payload)).subarray(0, 4)]),
);
const filePassword = `file-${randomBytes(12).toString("base64url")}`;
const exportPassword = `export-${randomBytes(12).toString("base64url")}`;
const secrets = {
  strings: [privateKey, filePassword, exportPassword],
  bytes: [[...scalar]],
  bytesRev: [[...scalar].reverse()],
};

const has = text => {
  if (typeof text !== "string" || !text) return false;
  const compact = text.replace(/\s/g, "");
  const candidates = [...secrets.strings];
  for (const value of [...secrets.bytes, ...secrets.bytesRev]) {
    const b = Buffer.from(value);
    candidates.push(
      b.toString("hex"),
      b.toString("hex").toUpperCase(),
      b.toString("base64"),
      b.toString("base64url"),
      [...b].join(","),
    );
  }
  return candidates.some(s => text.includes(s) || compact.includes(s));
};

function mina(args, password) {
  return execFileSync(env.MINA_BIN, args, {
    env: { PATH: env.PATH, HOME: env.KEY_DIR, MINA_PRIVKEY_PASS: password },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

const nativeFile = join(env.KEY_DIR, "native-key");
execFileSync(env.MINA_BIN, ["wallet", "generate", "--output", nativeFile], {
  env: {
    PATH: env.PATH,
    HOME: env.KEY_DIR,
    MINA_SEC_KEY: privateKey,
    MINA_PRIVKEY_PASS: filePassword,
  },
  stdio: "ignore",
});
const publicKey = readFileSync(`${nativeFile}.pub`, "utf8").trim();
summary.native = { publicKey };

const leaks = { console: 0, requests: 0, storage: 0, screen: 0 };
summary.leaks = leaks;
function writeConsole(tag, line) {
  if (has(line)) leaks.console++;
  const safe = has(line) ? "[a line with a secret, withheld]" : line;
  appendFileSync(consoleFile, `${new Date().toISOString()} [${tag}] ${safe}\n`);
}

const browser = await puppeteer.launch({
  executablePath: env.CHROME,
  userDataDir: env.PROFILE_DIR,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu"],
  defaultViewport: { width: 1280, height: 1000 },
  protocolTimeout: 180000,
});
const downloads = join(env.KEY_DIR, "downloads");
mkdirSync(downloads, { mode: 0o700 });
const browserCdp = await browser.target().createCDPSession();
await browserCdp.send("Browser.setDownloadBehavior", {
  behavior: "allow",
  downloadPath: downloads,
});

const page = await browser.newPage();
const workerProbe = { hits: {}, inspected: {}, canary: {} };
summary.workerProbe = workerProbe;
const canary = `minaweb-probe-canary-${randomBytes(16).toString("hex")}`;
const probeConfig = { ...secrets, canary };
const workers = await watchWorkers(page, () => probeConfig, workerProbe);
await page.evaluateOnNewDocument(p => {
  globalThis.__minawebProbe = p;
}, probeConfig);
await page.evaluateOnNewDocument(PROBE);
const csp = { console: [], reported: [] };
summary.csp = csp;
await watchCsp(page, text => {
  if (csp.console.length < 100)
    csp.console.push(has(text) ? "[withheld]" : text.slice(0, 500));
});
page.on("console", m => writeConsole("page", `[${m.type()}] ${m.text()}`));
page.on("pageerror", e => writeConsole("pageerror", String(e?.stack ?? e)));
page.on("workercreated", w =>
  w.on("console", m => writeConsole("worker", `[${m.type()}] ${m.text()}`)),
);
page.on("request", r => {
  if (has(r.url()) || has(r.postData() ?? "")) leaks.requests++;
});

async function shot(name) {
  if (await page.$("[data-private-key-shown]"))
    throw new Error(`refusing to screenshot ${name} with the key shown`);
  await page.screenshot({ path: join(out, `${name}.png`), fullPage: true });
  summary.shots.push(`${name}.png`);
  save();
}

async function clickText(text) {
  const ok = await page.evaluate(t => {
    const b = [...document.querySelectorAll("button")].find(
      x => x.textContent.trim() === t,
    );
    if (!b || b.disabled) return false;
    b.click();
    return true;
  }, text);
  if (!ok) throw new Error(`no button "${text}"`);
}

async function type(selector, text) {
  await page.click(selector, { clickCount: 3 });
  await page.keyboard.press("Backspace");
  await page.type(selector, text);
}

/** Everything the page can read back from its own storage, and its text. */
const pageState = () =>
  page.evaluate(async () => {
    const dbs = (await indexedDB.databases?.()) ?? [];
    return JSON.stringify({
      local: { ...localStorage },
      session: { ...sessionStorage },
      dbs: dbs.map(d => d.name),
      text: document.body.innerText,
      html: document.body.innerHTML,
    });
  });

async function openKeyFileImport() {
  await page.waitForFunction(
    () =>
      [...document.querySelectorAll("button")].some(
        b => b.textContent.trim() === "Import a devnet key instead",
      ),
    { timeout: 180000 },
  );
  await clickText("Import a devnet key instead");
  await page.click('input[type="checkbox"]');
  await clickText("Key file");
  await page.waitForSelector("#import-key-file");
}

/** Decrypts with this password; the page thread's longest gap between
 * 10 ms ticks while it runs, and the time to the key or the error. */
async function decrypt(password, want) {
  await type("#import-key-file-password", password);
  await page.evaluate(() => {
    const lag = (globalThis.__lag = { max: 0, last: performance.now() });
    lag.timer = setInterval(() => {
      const now = performance.now();
      lag.max = Math.max(lag.max, now - lag.last);
      lag.last = now;
    }, 10);
  });
  const t0 = Date.now();
  await clickText("Decrypt and import");
  await page.waitForSelector(want, { timeout: 120000 });
  const ms = Date.now() - t0;
  const maxGapMs = await page.evaluate(() => {
    clearInterval(globalThis.__lag.timer);
    return Math.round(globalThis.__lag.max);
  });
  return { ms, maxGapMs };
}

const publicOnPage = () =>
  page.$eval("[data-key-public]", e => e.getAttribute("data-key-public"));

try {
  csp.policy = await servedPolicy(env.BASE);
  await page.goto(`${env.BASE}/`, { waitUntil: "domcontentloaded" });
  // The start screen, if this build has one.
  await page
    .waitForSelector('[data-action="start-node"]', { timeout: 30000 })
    .then(() => page.click('[data-action="start-node"]'))
    .catch(() => {});
  // The positive control, before any key is in the page.
  await page.evaluate(exerciseProbe, canary);
  // The node's pages open before the sync ends, once the node answers.
  await page.waitForFunction(
    () =>
      document.querySelector('nav a[href="/key"]') ||
      [...document.querySelectorAll("button")].some(b =>
        b.textContent.includes("Open the node's pages now"),
      ),
    { timeout: 300000 },
  );
  if (!(await page.$('nav a[href="/key"]')))
    await clickText("Open the node's pages now");
  await page.waitForSelector('nav a[href="/key"]', { timeout: 30000 });
  await page.click('nav a[href="/key"]');

  // 2. The import.
  await openKeyFileImport();
  const fileInput = await page.$("#import-key-file");
  await fileInput.uploadFile(nativeFile);
  await page.waitForFunction(
    () => document.getElementById("import-key-file-text")?.value.length > 100,
  );
  await shot("keyfile-import");
  summary.wrongPassword = await decrypt(
    "not-the-password",
    "[data-import-key-file-error]",
  );
  summary.wrongPassword.message = await page.$eval(
    "[data-import-key-file-error]",
    e => e.textContent,
  );
  await shot("keyfile-wrong-password");
  summary.import = await decrypt(filePassword, "[data-key-public]");
  summary.import.publicKeyMatches = (await publicOnPage()) === publicKey;
  await shot("keyfile-imported");
  if (has(await pageState())) leaks.storage++;

  // 3. The export, read back by the native node.
  await type("#export-password", exportPassword);
  await type("#export-again", exportPassword);
  const t0 = Date.now();
  await clickText("Download key file");
  await page.waitForSelector("[data-export-done]", { timeout: 120000 });
  summary.export = { ms: Date.now() - t0 };
  let exported = null;
  for (let i = 0; i < 50 && !exported; i++) {
    exported = readdirSync(downloads).find(
      f => f.startsWith("mina-key-") && !f.endsWith(".crdownload"),
    );
    if (!exported) await sleep(200);
  }
  if (!exported) throw new Error("no download");
  const exportedPath = join(downloads, exported);
  const exportedText = readFileSync(exportedPath, "utf8");
  summary.export.file = exported;
  summary.export.holdsKey = has(exportedText.replace(exportPassword, ""));
  summary.export.format = Object.keys(JSON.parse(exportedText)).join(",");
  // The native node refuses a key file others can read, as OCaml does.
  execFileSync("chmod", ["600", exportedPath]);
  summary.export.nativeReads =
    mina(["wallet", "address", "--from", exportedPath], exportPassword)
      .trim()
      .split("\n")
      .includes(publicKey);
  await shot("keyfile-exported");
  if (has(await pageState())) leaks.storage++;

  // 4. A key file the OCaml node wrote.
  if (env.OCAML_FILE) {
    await clickText("Forget the key in this tab");
    await clickText("Forget it");
    await openKeyFileImport();
    await (await page.$("#import-key-file")).uploadFile(env.OCAML_FILE);
    await page.waitForFunction(
      () => document.getElementById("import-key-file-text")?.value.length > 100,
    );
    summary.ocaml = await decrypt(env.OCAML_PASSWORD, "[data-key-public]");
    summary.ocaml.publicKeyMatches =
      (await publicOnPage()) ===
      readFileSync(`${env.OCAML_FILE}.pub`, "utf8").trim();
  }
  if (has(await pageState())) leaks.storage++;
  summary.flowCompleted = true;
} catch (error) {
  summary.fatal = has(String(error))
    ? "[an error with a secret, withheld]"
    : String(error?.stack ?? error);
  await shot("fatal").catch(() => {});
} finally {
  await workers.drain();
  await page.evaluate(() => globalThis.__probeDrain?.()).catch(() => {});
  const frames = await page
    .evaluate(() => ({
      hits: globalThis.__probeHits,
      inspected: globalThis.__probeSeen,
      canary: globalThis.__probeCanary,
    }))
    .catch(error => ({ errors: [String(error)] }));
  summary.probe = { frames };
  const c = frames.canary ?? {};
  summary.controlSeen = ["fetch-url", "storage-write", "indexeddb-write"].every(
    k => c[k] > 0,
  );
  if (existsSync(consoleFile) && has(readFileSync(consoleFile, "utf8")))
    leaks.consoleFile = 1;
  summary.leakFree =
    probePassed(frames) &&
    probePassed(workerProbe) &&
    Object.values(leaks).every(n => n === 0);
  csp.reported = reportedViolations(env.CSP_REPORTS);
  summary.cspClean =
    Boolean(csp.policy) && csp.console.length === 0 && csp.reported.length === 0;
  summary.ok = Boolean(
    summary.flowCompleted &&
      summary.import?.publicKeyMatches &&
      summary.export?.nativeReads &&
      summary.export?.holdsKey === false &&
      (!env.OCAML_FILE || summary.ocaml?.publicKeyMatches) &&
      summary.wrongPassword?.message?.startsWith("Wrong password") &&
      summary.leakFree &&
      summary.controlSeen &&
      summary.cspClean,
  );
  if (!summary.ok) process.exitCode = 1;
  summary.ended = new Date().toISOString();
  secrets.strings.length = 0;
  save();
  await browser.close();
  rmSync(env.KEY_DIR, { recursive: true, force: true });
  console.log(summary.ok ? "PASS" : "FAIL", join(out, "summary.json"));
}
