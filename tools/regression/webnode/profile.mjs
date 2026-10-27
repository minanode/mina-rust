// Attach to a running web node browser, CPU-profile every worker for a few
// seconds and print the functions with the most self time, per worker. Each
// profile is also written to OUT/profile-<n>.cpuprofile, which Chrome's
// DevTools can open. See ../README.md.
//
//   RUN_DIR=<run.mjs's OUT> node profile.mjs [seconds] [top]
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { attach, loadPuppeteer, outDir } from "./lib.mjs";

const browser = await attach(loadPuppeteer(), 120000);
const seconds = Number(process.argv[2] ?? 10);
const top = Number(process.argv[3] ?? 15);
const out = outDir();

// Workers (including the WASM threads) are only reachable through the
// browser-level target list.
const root = await browser.target().createCDPSession();
const { targetInfos } = await root.send("Target.getTargets");
const sessions = [];
for (const info of targetInfos.filter((t) => t.type === "worker" || t.type === "page")) {
  const { sessionId } = await root.send("Target.attachToTarget", { targetId: info.targetId, flatten: true });
  const session = root.connection().session(sessionId);
  // Threads blocked in Atomics.wait (idle pool threads) never answer.
  const started = await Promise.race([
    (async () => {
      await session.send("Profiler.enable");
      await session.send("Profiler.setSamplingInterval", { interval: 500 });
      await session.send("Profiler.start");
      return true;
    })().catch(() => false),
    new Promise((r) => setTimeout(() => r(false), 3000)),
  ]);
  if (started) sessions.push({ url: info.url, type: info.type, session });
}
await new Promise((r) => setTimeout(r, seconds * 1000));
const report = [];
for (const [i, { url, type, session }] of sessions.entries()) {
  const stopped = await Promise.race([
    session.send("Profiler.stop").catch(() => null),
    new Promise((r) => setTimeout(() => r(null), 10000)),
  ]);
  if (!stopped) continue;
  const { profile } = stopped;
  const byId = new Map(profile.nodes.map((n) => [n.id, n]));
  const counts = new Map();
  for (const id of profile.samples) counts.set(id, (counts.get(id) ?? 0) + 1);
  const self = new Map();
  let total = 0;
  for (const [id, c] of counts) {
    const name = byId.get(id).callFrame.functionName || "(anonymous)";
    if (name === "(idle)" || name === "(program)") continue;
    self.set(name, (self.get(name) ?? 0) + c);
    total += c;
  }
  // The share of samples that were neither idle nor outside JS and WASM.
  const busy = total / Math.max(profile.samples.length, 1);
  report.push({
    i,
    type,
    url: url.slice(0, 60),
    busy: Math.round(busy * 100),
    top: [...self]
      .sort((a, b) => b[1] - a[1])
      .slice(0, top)
      .map(([n, c]) => [Math.round((c / Math.max(total, 1)) * 1000) / 10, n.replace(/\[[0-9a-f]{16}\]/g, "").slice(0, 150)]),
  });
  writeFileSync(join(out, `profile-${i}.cpuprofile`), JSON.stringify(profile));
}
console.log(report.map((r) => `${r.type} ${r.i} busy=${r.busy}% ${r.url}`).join("\n"));
for (const r of report.filter((r) => r.busy > 5)) {
  console.log(`\n== ${r.type} ${r.i} busy=${r.busy}% ${r.url}`);
  for (const [pct, name] of r.top) console.log(`${String(pct).padStart(5)}%  ${name}`);
}
browser.disconnect();
