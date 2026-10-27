// Attach to a running web node browser, evaluate an expression in the page
// with `node` bound to `window.webnode`, and print the result as JSON.
// See ../README.md.
//
//   RUN_DIR=<run.mjs's OUT> node peek.mjs ['node.stats().sync(1)'] [max chars]
import { attach, loadPuppeteer, webnodePage } from "./lib.mjs";

const browser = await attach(loadPuppeteer());
const page = await webnodePage(browser);
const out = await page.evaluate(async (expr) => {
  const race = (p) => Promise.race([p, new Promise((r) => setTimeout(() => r("timeout"), 20000))]);
  // eslint-disable-next-line no-unused-vars
  const node = window.webnode;
  return JSON.stringify(await race(eval(expr)));
}, process.argv[2] ?? "node.stats().sync(1)");
console.log(String(out).slice(0, Number(process.argv[3] ?? 4000)));
browser.disconnect();
