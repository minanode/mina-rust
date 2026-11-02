// Start a real worker role, then verify that each bridge initialization
// failure reaches the start-up screen and stops its owned worker. Run in a
// browser slot, with BASE, OUT, CHROME and PUPPETEER_DIR as the other checks.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { findPuppeteer } from './lib.mjs';

const { BASE, OUT, CHROME } = process.env;
for (const [name, value] of Object.entries({ BASE, OUT, CHROME })) {
  if (!value) throw new Error(`set ${name}`);
}
const { puppeteer } = findPuppeteer();
mkdirSync(OUT, { recursive: true });
const syncTimeout = Number(process.env.SYNC_MIN ?? 30) * 60000;
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true,
  args: ['--no-sandbox', '--disable-gpu'], protocolTimeout: syncTimeout + 60000 });
const result = { cases: {} };
let activePage;
const save = () => writeFileSync(join(OUT, 'bridge-startup.json'), JSON.stringify(result, null, 2));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
try {
  const page = activePage = await browser.newPage();
  let ready = false;
  page.on('console', message => {
    if (message.text().includes('isolated block prover ready')) ready = true;
  });
  await page.goto(BASE, { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.waitForFunction(() => Boolean(globalThis.webnode), { timeout: 180000 });
  // Generate a throwaway key in the page. Only its public half leaves it.
  const publicKey = await page.evaluate(async () => {
    const glue = await import('/assets/webnode/pkg/mina_node_web.js');
    const key = glue.generate_account_key();
    sessionStorage.setItem('minaweb.role', JSON.stringify({ kind: 'worker', publicKey: key.publicKey, fee: '1' }));
    return key.publicKey;
  });
  result.commit = await page.evaluate(async () => (await (await fetch('/build-info.json')).json()).commit);
  if (process.env.EXPECTED_COMMIT && result.commit !== process.env.EXPECTED_COMMIT) {
    throw new Error('the served build is not the expected commit');
  }
  const began = Date.now();
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.waitForFunction(() => {
    const state = document.documentElement.dataset.boot;
    if (state?.startsWith('failed:')) throw new Error(document.querySelector('[data-failure]')?.textContent);
    return state === 'synced';
  }, { timeout: syncTimeout, polling: 1000 });
  if (!ready) throw new Error('worker role synced without a ready prover');
  result.positive = { syncedSeconds: (Date.now() - began) / 1000, ready, rpc: true, publicKey };
  await page.screenshot({ path: join(OUT, 'bridge-startup-positive.png'), fullPage: true });
  await page.close();
  save();

  for (const [mode, expected] of [
    ['worker-error', 'injected bridge worker error'],
    ['message-error', 'prover bridge worker message error'],
    ['rejected-init', 'prover bridge worker initialization failed'],
    ['timeout', 'prover bridge initialization timed out'],
  ]) {
    const context = await browser.createBrowserContext();
    const failedPage = activePage = await context.newPage();
    await failedPage.evaluateOnNewDocument((mode, publicKey) => {
      sessionStorage.setItem('minaweb.role', JSON.stringify({ kind: 'worker', publicKey, fee: '1' }));
      const facts = globalThis.__bridgeFailureTest = { injected: 0, terminated: 0, armedBeforeWorker: false, bridgeTimer: false };
      const nativeTimer = globalThis.setTimeout.bind(globalThis);
      globalThis.setTimeout = (callback, ms, ...args) => {
        if (String(callback).includes('prover bridge initialization timed out')) {
          facts.bridgeTimer = true;
          if (mode === 'timeout') ms = 250;
        }
        return nativeTimer(callback, ms, ...args);
      };
      const NativeWorker = globalThis.Worker;
      globalThis.Worker = class extends NativeWorker {
        constructor(url, options) {
          const bridge = String(options?.name ?? '').split('\n').at(-1) === 'mina_block_prover';
          if (bridge) {
            facts.injected++;
            facts.armedBeforeWorker = facts.bridgeTimer;
            let script = 'self.onmessage = () => {};';
            if (mode === 'worker-error') script = 'throw new Error("injected bridge worker error")';
            if (mode !== 'rejected-init') {
              url = URL.createObjectURL(new Blob([script], { type: 'application/javascript' }));
            }
          }
          super(url, options);
          this.bridgeTestOwned = bridge;
          if (bridge && mode === 'message-error') {
            queueMicrotask(() => this.dispatchEvent(new MessageEvent('messageerror')));
          }
        }
        postMessage(message, ...rest) {
          if (this.bridgeTestOwned && mode === 'rejected-init' && Array.isArray(message)) {
            // Exercise wasm_thread's real init().catch() path, not a mock
            // rejection: the module bytes cannot form a WebAssembly module.
            message = [...message];
            message[0] = new Uint8Array([0, 0, 0, 0]);
            facts.rejectedInit = true;
          }
          return super.postMessage(message, ...rest);
        }
        terminate() {
          if (this.bridgeTestOwned) facts.terminated++;
          return super.terminate();
        }
      };
    }, mode, publicKey);
    try {
      await failedPage.goto(BASE, { waitUntil: 'domcontentloaded', timeout: 60000 });
      await failedPage.waitForFunction(() => document.documentElement.dataset.boot === 'failed:start', { timeout: 90000 });
      await sleep(250);
      const observed = await failedPage.evaluate(() => ({
        ...globalThis.__bridgeFailureTest,
        state: document.documentElement.dataset.boot,
        text: document.querySelector('[data-failure]')?.textContent ?? '',
        rpc: Boolean(globalThis.webnode),
        hosts: document.querySelectorAll('iframe[data-mina-block-prover]').length,
      }));
      observed.ok = observed.injected === 1 && observed.armedBeforeWorker && observed.terminated >= 1 &&
        !observed.rpc && observed.hosts === 0 && observed.text.includes(expected) &&
        (mode !== 'rejected-init' || observed.rejectedInit === true);
      result.cases[mode] = observed;
      await failedPage.screenshot({ path: join(OUT, `bridge-startup-${mode}.png`), fullPage: true });
      save();
      if (!observed.ok) throw new Error(`${mode} did not fail specifically or clean up`);
    } finally {
      await context.close();
    }
  }
  result.ok = true;
} catch (error) {
  result.ok = false;
  result.error = String(error);
  if (activePage && !activePage.isClosed()) {
    result.failure = await activePage.evaluate(() => ({
      state: document.documentElement.dataset.boot,
      text: document.querySelector('[data-failure]')?.textContent ?? '',
    })).catch(() => null);
    await activePage.screenshot({ path: join(OUT, 'startup-unexpected-failure.png'), fullPage: true }).catch(() => {});
  }
  process.exitCode = 1;
} finally {
  save();
  await browser.close();
}
