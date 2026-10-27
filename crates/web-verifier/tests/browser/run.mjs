// Drive the demo page in headless Chrome and print the setup timings and results.
//
//   CHROME=/path/to/chrome-headless-shell node run.mjs [url] [source] [arg]
//
// source `bucket`: verify the latest `arg` heights of mainnet from the archive
//   (NETWORK=devnet for devnet) and run the tamper test.
// source `live`: verify gossiped blocks from the relay at RELAY (default
//   ws://127.0.0.1:8741/blocks) until `arg` blocks have verified.
//
// Serve the page first with `python3 crates/web-verifier/www/serve.py`.
import puppeteer from "puppeteer-core";

const url = process.argv[2] ?? "http://127.0.0.1:8740/";
const source = process.argv[3] ?? "bucket";
const count = process.argv[4] ?? "5";
const network = process.env.NETWORK ?? "mainnet";
const relay = process.env.RELAY ?? "ws://127.0.0.1:8741/blocks";
const timeoutMs = Number(process.env.TIMEOUT_MS ?? 600_000);

const browser = await puppeteer.launch({
  executablePath: process.env.CHROME,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu"],
  protocolTimeout: timeoutMs + 60_000,
});

try {
  const page = await browser.newPage();
  page.on("console", (msg) => console.log(`[console.${msg.type()}] ${msg.text()}`));
  page.on("pageerror", (err) => console.log(`[pageerror] ${err.message}`));

  await page.goto(url, { waitUntil: "load" });
  const isolated = await page.evaluate(() => crossOriginIsolated);
  console.log(`crossOriginIsolated: ${isolated}, cores: ${await page.evaluate(() => navigator.hardwareConcurrency)}`);

  await page.select("#source", source);
  await page.evaluate(() => document.getElementById("source").dispatchEvent(new Event("change")));
  const started = Date.now();
  if (source === "live") {
    await page.$eval("#relay", (el, value) => { el.value = value; }, relay);
    await page.click("#verify");
    await page.waitForFunction(
      (n) => [...document.querySelectorAll("#blocks tbody tr")]
        .filter((tr) => !tr.cells[5].classList.contains("pending")).length >= n,
      { timeout: timeoutMs, polling: 1000 },
      Number(count),
    );
  } else {
    await page.select("#network", network);
    await page.$eval("#count", (el, value) => { el.value = value; }, count);
    await page.click("#verify");
    await page.waitForFunction(
      () => document.getElementById("summary").textContent.includes("verified"),
      { timeout: timeoutMs, polling: 1000 },
    );
  }
  console.log(`\nrun took ${((Date.now() - started) / 1000).toFixed(1)} s end to end`);

  if (source === "bucket" && await page.$eval("#tamper", (el) => !el.disabled)) {
    const rows = await page.$$eval("#blocks tbody tr", (r) => r.length);
    await page.click("#tamper");
    await page.waitForFunction(
      (n) => {
        const rows = document.querySelectorAll("#blocks tbody tr");
        return rows.length > n && !rows[0].cells[5].classList.contains("pending");
      },
      { timeout: timeoutMs, polling: 500 },
      rows,
    );
  }

  const report = await page.evaluate(() => ({
    tip: document.getElementById("tip").innerText.replace(/\n+/g, " | "),
    setup: [...document.querySelectorAll("#setup li")].map((li) => li.textContent),
    blocks: [...document.querySelectorAll("#blocks tbody tr")].map((tr) =>
      [...tr.cells].map((td) => td.textContent).join(" | "),
    ),
    summary: document.getElementById("summary").textContent,
  }));
  console.log(`\nBest tip: ${report.tip}`);
  console.log("\nSetup:");
  for (const line of report.setup) console.log(`  ${line}`);
  console.log("\nBlocks:");
  for (const line of report.blocks) console.log(`  ${line}`);
  console.log(`\n${report.summary}`);

  if (process.env.SCREENSHOT) {
    await page.setViewport({ width: 1100, height: 900, deviceScaleFactor: 2 });
    await page.screenshot({ path: process.env.SCREENSHOT, fullPage: true });
    console.log(`screenshot: ${process.env.SCREENSHOT}`);
  }
} finally {
  await browser.close();
}
