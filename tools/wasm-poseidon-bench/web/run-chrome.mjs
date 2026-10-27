// Opens index.html in headless Chrome and prints what it logs.
//
//   CHROME=... LD_LIBRARY_PATH=... node web/run-chrome.mjs <base-url> <modules> [filter]
//
// R8_JS_FLAGS is passed to V8, e.g. "--no-liftoff" to compile everything with
// TurboFan up front (steady state without tier-up noise).
// e.g. node web/run-chrome.mjs http://127.0.0.1:4215 plain.wasm,simd.wasm
// puppeteer-core is taken from the web-verifier browser tests
// (`npm ci` in crates/web-verifier/tests/browser). R8_PUPPETEER_FROM points at
// another package.json whose node_modules has it, e.g. the main checkout's
// when running from a worktree.
import { createRequire } from "node:module";
import { mkdtempSync, rmSync } from "node:fs";

const require = createRequire(
  process.env.R8_PUPPETEER_FROM ??
    new URL(
      "../../../crates/web-verifier/tests/browser/package.json",
      import.meta.url,
    ),
);
const puppeteer = require("puppeteer-core");

const [base, modules, filter] = process.argv.slice(2);
// Own profile directory, so other agents' attach scripts (which pick the
// newest puppeteer profile in /tmp) don't latch onto this browser.
const userDataDir = mkdtempSync("/tmp/r8-chrome-");
const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  userDataDir,
  args: [
    "--no-sandbox",
    ...(process.env.R8_JS_FLAGS
      ? [`--js-flags=${process.env.R8_JS_FLAGS}`]
      : []),
  ],
});
try {
  const page = await browser.newPage();
  page.on("console", m => console.log(m.text()));
  const url = `${base}/index.html?wasm=${modules}${filter ? `&filter=${encodeURIComponent(filter)}` : ""}`;
  await page.goto(url);
  await page.waitForFunction("window.__r8 !== undefined", {
    timeout: 0,
    polling: 1000,
  });
  const r = await page.evaluate("window.__r8");
  if (!r.ok) process.exitCode = 1;
} finally {
  await browser.close();
  rmSync(userDataDir, { recursive: true, force: true });
}
