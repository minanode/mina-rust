// Run in a remote browser slot. MODULES contains ice.js (the page's ice.ts
// compiled without changes) and transport.js (the shipped web.js). ISSUER is
// a disposable short-TTL coturn issuer; never log its response or credentials.
// This tests real forced-TURN data channels, not Mina chain synchronization.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { findPuppeteer } from "./lib.mjs";

const { OUT, MODULES, ISSUER, CHROME, STATIC_PORT } = process.env;
for (const value of [OUT, MODULES, ISSUER, CHROME, STATIC_PORT]) assert.ok(value, "missing test configuration");
mkdirSync(OUT, { recursive: true });
const server = createServer((req, res) => {
  res.setHeader("Cache-Control", "no-store");
  if (req.url === "/") { res.end("<!doctype html><title>TURN renewal check</title>"); return; }
  if (!["/ice.js", "/transport.js", "/ice-url-cases.json"].includes(req.url)) { res.writeHead(404).end(); return; }
  res.setHeader("Content-Type", req.url.endsWith(".json") ? "application/json" : "text/javascript");
  res.end(readFileSync(join(MODULES, req.url.slice(1))));
});
await new Promise(resolve => server.listen(Number(STATIC_PORT), "127.0.0.1", resolve));
let browser;
try {
  browser = await findPuppeteer().puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"], protocolTimeout: 180000 });
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${STATIC_PORT}/`);
  const result = await page.evaluate(async issuer => {
    const { loadIceServers, renewIceServers, iceExpiry } = await import("/ice.js");
    const { createWebrtcConnection, updateWebrtcIceServers } = await import("/transport.js");
    const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
    const deadline = async (predicate, label) => {
      const end = Date.now() + 30000;
      while (!(await predicate())) {
        if (Date.now() > end) throw new Error(label);
        await sleep(100);
      }
    };
    const cases = await (await fetch("/ice-url-cases.json")).json();
    for (const url of cases.valid) {
      let probe;
      try { probe = new RTCPeerConnection({ iceServers: [{ urls: [url], username: "fixture", credential: "fixture" }] }); }
      catch { throw new Error("supported ICE URL rejected by browser"); }
      finally { probe?.close(); }
    }
    const first = await loadIceServers(issuer, location.href);
    if (!first) throw new Error("issuer unavailable");
    const originalExpiry = iceExpiry(first);
    const ttl = originalExpiry - Date.now();
    if (!(ttl > 0 && ttl <= 30000)) throw new Error("issuer TTL is not short");
    let renewals = 0;
    let requests = 0;
    let latestExpiry = originalExpiry;
    let outage = false;
    const invalidResponses = [[], [{ ...first[0], urls: ["turn:"] }]];
    const load = async (...args) => {
      requests++;
      return invalidResponses.length ? invalidResponses.shift() : outage ? null : loadIceServers(...args);
    };
    const stop = renewIceServers(issuer, location.href, first, servers => {
      updateWebrtcIceServers(servers);
      const expiry = iceExpiry(servers);
      if (expiry) { renewals++; latestExpiry = expiry; }
    }, load);
    const peers = [];
    let messages = 0;
    const pair = async () => {
      const a = createWebrtcConnection({ iceServers: first, iceTransportPolicy: "relay" });
      const b = createWebrtcConnection({ iceServers: first, iceTransportPolicy: "relay" });
      peers.push(a, b);
      a.onicecandidate = event => { if (event.candidate) b.addIceCandidate(event.candidate).catch(() => {}); };
      b.onicecandidate = event => { if (event.candidate) a.addIceCandidate(event.candidate).catch(() => {}); };
      const channel = a.createDataChannel("renewal");
      let received;
      b.ondatachannel = event => { received = event.channel; received.onmessage = () => messages++; };
      await a.setLocalDescription(await a.createOffer());
      await b.setRemoteDescription(a.localDescription);
      await b.setLocalDescription(await b.createAnswer());
      await a.setRemoteDescription(b.localDescription);
      await deadline(() => channel.readyState === "open" && received?.readyState === "open", "TURN channel did not open");
      const stats = await a.getStats();
      const selected = [...stats.values()].find(s => s.type === "candidate-pair" && s.state === "succeeded" && s.nominated);
      if (!selected || stats.get(selected.localCandidateId)?.candidateType !== "relay") throw new Error("selected pair is not TURN");
      return channel;
    };
    try {
      const channel = await pair();
      updateWebrtcIceServers(first);
      const invalid = [[], ...cases.invalid.map(url => [{ ...first[0], urls: [url] }]),
        [{ ...first[0], username: "" }], [{ ...first[0], credential: "" }],
        [{ ...first[0], username: "999999999999:far" }]];
      for (const servers of invalid) {
        let rejected = false;
        try { updateWebrtcIceServers(servers); } catch { rejected = true; }
        if (!rejected) throw new Error("invalid renewal accepted");
      }
      const afterBad = await pair();
      const beforeBad = messages;
      afterBad.send("new connection after bad renewal");
      await deadline(() => messages > beforeBad, "bad renewal poisoned new connection");
      const send = async () => {
        const before = messages;
        channel.send("still connected");
        await deadline(() => messages > before, "existing channel lost during renewal");
      };
      await deadline(async () => {
        await send();
        return renewals >= 2 && Date.now() > originalExpiry + 1000;
      }, "two renewals did not cross original expiry");
      const afterExpiry = await pair();
      const before = messages;
      afterExpiry.send("new connection after original expiry");
      await deadline(() => messages > before, "new channel did not carry data");
      outage = true;
      const outageExpiry = latestExpiry;
      await deadline(async () => { await send(); return Date.now() > outageExpiry + 1000; }, "outage did not cross expiry");
      const previousRenewals = renewals;
      outage = false;
      await deadline(() => renewals > previousRenewals, "issuer recovery did not renew");
      const recovered = await pair();
      recovered.send("recovered connection");
      await send();
      return { ok: true, ttlMs: ttl, renewals, requests, messages, forcedTurnPairs: 4, invalidRenewalsRejected: invalid.length, invalidIssuerResponsesRetried: 2,
        originalExpiryCrossed: true, existingConnectionSurvivedOutage: true, issuerRecovery: true };
    } finally { stop(); for (const peer of peers) peer.close(); }
  }, ISSUER);
  writeFileSync(join(OUT, "turn-renewal.json"), JSON.stringify(result, null, 2) + "\n");
  console.log(JSON.stringify(result));
} catch (error) {
  // Browser errors contain only the fixed failure labels above.
  writeFileSync(join(OUT, "turn-renewal.json"), JSON.stringify({ ok: false, error: String(error) }) + "\n");
  process.exitCode = 1;
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}
