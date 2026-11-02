// Attach only to the browser started by run.mjs in RUN_DIR. Create independent
// browser contexts, so each node has its own identity and persistent storage.
// BASE is the test build's origin. CLIENTS defaults to 20 honest nodes; add two
// channel-zero flooders and two peers that withhold authentication (override
// FLOOD_CLIENTS and ABANDON_CLIENTS for smaller runs). This is an
// opt-in local relay test, never a driver for public third-party relays.
//
// RUN_DIR=... BASE=http://127.0.0.1:5910 OUT=... node relay-soak.mjs
// SOAK_SECONDS defaults to 900. BAD_AFTER_SECONDS defaults to 300, leaving a
// normal-traffic baseline. Set MIN_SYNC_HEIGHT to the synced relay height to
// exclude initial genesis readiness.
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { browserEndpoint, findPuppeteer, outDir } from './lib.mjs';

if (!process.env.RUN_DIR || !process.env.BASE) {
  throw new Error('RUN_DIR and BASE are required; start run.mjs first');
}
const clients = Number(process.env.CLIENTS ?? 20);
const flooders = Number(process.env.FLOOD_CLIENTS ?? 2);
const abandoners = Number(process.env.ABANDON_CLIENTS ?? 2);
const duration = Number(process.env.SOAK_SECONDS ?? 900);
const badAfter = Number(process.env.BAD_AFTER_SECONDS ?? 300);
const minHeight = Number(process.env.MIN_SYNC_HEIGHT ?? 2);
if (![clients, flooders, abandoners, duration, badAfter, minHeight].every(Number.isSafeInteger) ||
    clients < 1 || flooders < 0 || abandoners < 0 || flooders + abandoners < 1 ||
    duration <= 0 || badAfter < 0 || badAfter >= duration || minHeight < 1) {
  throw new Error('use positive clients/duration/height, at least one attacker and an attack before the end');
}
const { puppeteer } = findPuppeteer();
const browser = await puppeteer.connect({ browserWSEndpoint: browserEndpoint(), protocolTimeout: 120000 });
const out = outDir();
mkdirSync(out, { recursive: true });
const records = [];
const started = Date.now();
const seconds = () => Math.round((Date.now() - started) / 1000);
const sampleFile = join(out, 'clients.jsonl');
writeFileSync(sampleFile, '');

async function startClient(kind, index) {
  const context = await browser.createBrowserContext();
  const page = await context.newPage();
  const record = { kind, index, context, page, synced: false, closed: false, honestDisconnected: false };
  records.push(record);
  await page.evaluateOnNewDocument(kind => {
    const Native = RTCPeerConnection;
    self.__soak = { peers: [] };
    self.RTCPeerConnection = function (config) {
      const pc = new Native(config);
      const entry = { pc, connected: false, disconnected: false, controlSent: 0, withheld: 0, floodSent: 0, channelClosed: false };
      self.__soak.peers.push(entry);
      pc.addEventListener('connectionstatechange', () => {
        if (pc.connectionState === 'connected') entry.connected = true;
        if (entry.connected && ['disconnected', 'failed', 'closed'].includes(pc.connectionState)) entry.disconnected = true;
      });
      const create = pc.createDataChannel.bind(pc);
      pc.createDataChannel = (label, options) => {
        const dc = create(label, options);
        if (options?.id === 0) {
          const send = dc.send.bind(dc);
          dc.send = data => {
            if (kind === 'abandon') { entry.withheld++; return; }
            send(data);
            entry.controlSent++;
          };
          dc.addEventListener('close', () => { entry.channelClosed = true; });
        }
        if (options?.id === 0 && kind === 'flood') {
          dc.addEventListener('open', () => setTimeout(() => {
            // Valid ready frames isolate the control-message admission limit.
            const ready = new Uint8Array([77, 78, 65, 82, 68, 89, 0, 3]);
            for (let i = 0; i < 4096 && dc.readyState === 'open'; i++) {
              try { dc.send(ready); entry.floodSent++; } catch { break; }
            }
          }, 5000));
        }
        return dc;
      };
      return pc;
    };
    self.RTCPeerConnection.prototype = Native.prototype;
    Object.setPrototypeOf(self.RTCPeerConnection, Native);
    // Keep each browser node modest; this is a relay concurrency test.
    Object.defineProperty(navigator, 'hardwareConcurrency', { get: () => 2 });
  }, kind);
  page.on('pageerror', error => appendFileSync(join(out, 'errors.log'), `${kind}/${index} ${error.message}\n`));
  await page.goto(process.env.BASE, { waitUntil: 'domcontentloaded', timeout: 120000 });
  return record;
}

async function sample(record) {
  if (record.closed) return;
  const result = await record.page.evaluate(async () => {
    const s = self.__soak;
    const node = self.webnode;
    const status = node?.status ? await Promise.race([
      node.status(), new Promise(resolve => setTimeout(() => resolve(null), 5000)),
    ]) : null;
    return {
      sync: status?.transition_frontier?.sync?.status ?? null,
      height: status?.transition_frontier?.best_tip?.height ?? null,
      peers: status?.peers?.filter(p => p.connection_status === 'Connected').length ?? null,
      open: s?.peers.filter(p => p.pc.connectionState === 'connected').length ?? 0,
      connections: s?.peers.map(({ pc, ...p }) => ({ ...p, state: pc.connectionState })) ?? [],
    };
  }).catch(error => ({ error: error.message }));
  record.synced ||= result.sync === 'Synced' && result.height >= minHeight;
  if (!record.identity) {
    record.identity = await record.page.evaluate(async () => {
      if (!self.webnode) return null;
      return Promise.race([self.webnode.state().get('$.p2p.Ready.config.identity_pub_key'),
        new Promise(resolve => setTimeout(() => resolve(null), 5000))]);
    }).catch(() => null);
  }
  result.identity = record.identity;
  record.last = result;
  record.honestDisconnected ||= record.kind === 'good' &&
    (result.connections?.some(p => p.connected && (p.disconnected || p.channelClosed)) ?? false);
  appendFileSync(sampleFile, JSON.stringify({ seconds: seconds(), kind: record.kind, index: record.index, ...result }) + '\n');
  const attacked = result.connections?.filter(p => p.connected && (p.floodSent > 0 || p.withheld > 0)) ?? [];
  if (record.kind !== 'good' && attacked.length > 0 && attacked.every(p => p.channelClosed)) {
    // Browser-side evidence only. Correlate these timestamps and identities
    // with the relay's peer states and overload counters before claiming teardown.
    record.closed = true;
    await record.context.close();
  }
}

try {
  // Stagger startup to separate handshake bursts from steady-state traffic.
  for (let i = 0; i < clients; i++) {
    await startClient('good', i);
    await new Promise(resolve => setTimeout(resolve, 1500));
  }
  let badStarted = false;
  while (seconds() < duration) {
    if (!badStarted && seconds() >= badAfter) {
      badStarted = true;
      for (const [kind, count] of [['flood', flooders], ['abandon', abandoners]]) {
        for (let i = 0; i < count; i++) await startClient(kind, i);
      }
    }
    await Promise.all(records.map(sample));
    console.log(JSON.stringify({ seconds: seconds(), goodSynced: records.filter(r => r.kind === 'good' && r.synced).length,
      honestDisconnected: records.filter(r => r.honestDisconnected).length,
      badClosed: records.filter(r => r.kind !== 'good' && r.closed).length }));
    await new Promise(resolve => setTimeout(resolve, 10000));
  }
  const summary = records.map(({ kind, index, synced, closed, honestDisconnected, last }) => ({ kind, index, synced, closed, honestDisconnected, last }));
  writeFileSync(join(out, 'summary.json'), JSON.stringify(summary, null, 2) + '\n');
  if (summary.filter(r => r.kind === 'good').some(r => r.honestDisconnected || !r.synced || r.last?.sync !== 'Synced' || !(r.last?.open > 0) || r.last?.height < minHeight) ||
      summary.filter(r => r.kind !== 'good').length !== flooders + abandoners || summary.filter(r => r.kind !== 'good').some(r => !r.closed)) {
    process.exitCode = 1;
  }
} finally {
  await Promise.all(records.filter(r => !r.closed).map(r => r.context.close().catch(() => {})));
  browser.disconnect();
}
