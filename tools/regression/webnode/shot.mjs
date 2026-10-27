// Attach to a running web node browser, screenshot the web node's page and
// print the start of its text. See ../README.md.
//
//   RUN_DIR=<run.mjs's OUT> node shot.mjs [file.png]
import { attach, loadPuppeteer, webnodePage } from "./lib.mjs";

const browser = await attach(loadPuppeteer());
const page = await webnodePage(browser);
await page.setViewport({ width: 1600, height: 1000 });
await new Promise((r) => setTimeout(r, 3000));
console.log(page.url());
await page.screenshot({ path: process.argv[2] ?? "webnode.png" });
console.log(await page.evaluate(() => document.body.innerText.slice(0, 800).replace(/\s+/g, " ")));
browser.disconnect();
