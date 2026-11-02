// Refuse a changed prover file on the page's start-up screen, before the
// node or its isolated prover starts. Run in a browser slot using the same
// served build and Chrome setup as minaweb-check.mjs.
// Environment: BASE, OUT, CHROME, PUBLIC_KEY (a throwaway worker public key).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { findPuppeteer } from "./lib.mjs";

for (const key of ["BASE", "OUT", "CHROME", "PUBLIC_KEY"]) {
  if (!process.env[key]) throw new Error(`set ${key}`);
}
const { BASE, OUT, CHROME, PUBLIC_KEY } = process.env;
const { puppeteer } = findPuppeteer();
mkdirSync(OUT, { recursive: true });
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu"],
  defaultViewport: { width: 1440, height: 960 },
  protocolTimeout: 120000,
});
const result = { tamperedRequests: 0, proverStarted: false };
try {
  const page = await browser.newPage();
  await page.evaluateOnNewDocument(publicKey => {
    sessionStorage.setItem("minaweb.role", JSON.stringify({ kind: "worker", publicKey, fee: "1" }));
  }, PUBLIC_KEY);
  page.on("console", message => {
    if (message.text().includes("isolated block prover ready")) result.proverStarted = true;
  });
  await page.setRequestInterception(true);
  page.on("request", async request => {
    try {
      if (!request.url().endsWith("_gates.json")) return await request.continue();
      const response = await fetch(request.url());
      if (!response.ok) throw new Error(`circuit download: HTTP ${response.status}`);
      const bytes = Buffer.from(await response.arrayBuffer());
      bytes[0] ^= 1;
      result.tamperedRequests++;
      result.file = new URL(request.url()).pathname.split("/").pop();
      await request.respond({
        status: 200,
        contentType: "application/json",
        headers: {
          "Access-Control-Allow-Origin": new URL(BASE).origin,
          "Cross-Origin-Resource-Policy": "cross-origin",
          "Cache-Control": "no-store",
        },
        body: bytes,
      });
    } catch (error) {
      result.interceptionError = String(error);
      await request.abort().catch(() => {});
    }
  });
  await page.goto(BASE, { waitUntil: "domcontentloaded", timeout: 60000 });
  await page.waitForFunction(() => document.documentElement.dataset.boot?.startsWith("failed:"), { timeout: 180000 });
  result.state = await page.evaluate(() => document.documentElement.dataset.boot);
  result.text = await page.$eval("[data-failure]", element => element.innerText);
  result.nodeStarted = await page.evaluate(() => Boolean(globalThis.webnode));
  result.hostCount = await page.evaluate(() => document.querySelectorAll('iframe[data-mina-block-prover]').length);
  result.ok = result.tamperedRequests === 1 && result.state === "failed:start" &&
    result.text.includes("Prover startup failed") &&
    result.text.includes("SHA-256") && !result.nodeStarted && !result.proverStarted && result.hostCount === 0;
  await page.screenshot({ path: join(OUT, "prover-tamper.png"), fullPage: true });
  if (!result.ok) throw new Error("tampered prover did not fail at start-up");
} catch (error) {
  result.ok = false;
  result.error = String(error);
  process.exitCode = 1;
} finally {
  writeFileSync(join(OUT, "prover-tamper.json"), JSON.stringify(result, null, 2));
  await browser.close();
}
