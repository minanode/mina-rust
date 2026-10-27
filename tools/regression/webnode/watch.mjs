// Attach to a running web node browser; every EVERY_MS print block sync
// progress, best tip and WASM memory, and print page errors as they happen.
// See ../README.md.
//
//   RUN_DIR=<run.mjs's OUT> node watch.mjs [minutes]
//
// Console lines that look like errors go to OUT/watch-console.log.
import { appendFileSync } from "node:fs";
import { join } from "node:path";
import { attach, loadPuppeteer, outDir, stamp, webnodePage } from "./lib.mjs";

const browser = await attach(loadPuppeteer());
const page = await webnodePage(browser);
const started = Date.now();
const t = () => stamp(started);
const errors = join(outDir(), "watch-console.log");
page.on("pageerror", (e) => console.log(`${t()} PAGEERROR ${(e.stack ?? e.message).slice(0, 1500)}`));
page.on("console", (m) => {
  if (/panicked|ERROR|error/.test(m.text()) && !/Cursed hack/.test(m.text()))
    appendFileSync(errors, `${t()} ${m.text().slice(0, 2000)}\n`);
});
const minutes = Number(process.argv[2] ?? 60);
while (Date.now() - started < minutes * 60000) {
  const s = await page
    .evaluate(async () => {
      const race = (p) => Promise.race([p, new Promise((r) => setTimeout(() => r(null), 20000))]);
      const node = window.webnode;
      if (!node) return { error: "no window.webnode yet" };
      const sync = await race(node.stats().sync(1));
      const status = await race(node.status());
      const counts = {};
      for (const b of sync?.[0]?.blocks ?? []) counts[b.status] = (counts[b.status] ?? 0) + 1;
      return {
        kind: sync?.[0]?.kind,
        synced: sync?.[0]?.synced,
        blocks: counts,
        best_tip: status?.transition_frontier?.best_tip?.height,
        sync: status?.transition_frontier?.sync?.status,
        peers: status?.peers?.filter((p) => p.connection_status === "Connected").length,
        wasm_mb: (self.__wasmMemories ?? []).map((m) => Math.round(m.buffer.byteLength / 2 ** 20)),
      };
    })
    .catch((e) => ({ error: e.message }));
  console.log(`${t()} ${JSON.stringify(s)}`);
  await new Promise((r) => setTimeout(r, Number(process.env.EVERY_MS ?? 30000)));
}
browser.disconnect();
