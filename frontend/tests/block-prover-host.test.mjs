import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import {
  pinnedWasm,
  verifiedWasm,
} from '../../crates/node/web/js/block_prover_host.js';

// W1 fix round: the prover's iframe instantiates the WASM again; with a
// pinned WASM it checks the bytes first. Its document embeds this function.
const bytes = new TextEncoder().encode('\0asm prover').buffer;
const sha256 = createHash('sha256').update(Buffer.from(bytes)).digest('hex');
const url = 'https://assets.minaweb.org/wasm/x.wasm';
const serve =
  (body, status = 200) =>
  () =>
    Promise.resolve({
      ok: status === 200,
      status,
      arrayBuffer: () => Promise.resolve(body),
    });

test('the prover takes the pinned bytes and refuses others', async () => {
  assert.equal(await verifiedWasm({ url, sha256 }, serve(bytes)), bytes);
  const other = new TextEncoder().encode('\0asm provef').buffer;
  await assert.rejects(verifiedWasm({ url, sha256 }, serve(other)), error => {
    assert.match(error.message, /not the one this page was built with/);
    assert.match(error.message, new RegExp(`expected ${sha256}`));
    return true;
  });
  await assert.rejects(
    verifiedWasm({ url, sha256 }, serve(bytes, 404)),
    /HTTP 404/,
  );
});

test('the pin comes from the page, and a bad one is fatal', () => {
  delete globalThis.__MINA_WEBNODE_WASM__;
  assert.equal(pinnedWasm(), null);
  globalThis.__MINA_WEBNODE_WASM__ = { url, sha256 };
  assert.deepEqual(pinnedWasm(), { url, sha256 });
  globalThis.__MINA_WEBNODE_WASM__ = { url, sha256: 'abc' };
  assert.throws(
    () => pinnedWasm(),
    error => error.fatal === true,
  );
  delete globalThis.__MINA_WEBNODE_WASM__;
});

// The bridge watchdog is independent of receiving a command or making a host.
const { BridgeStartup } = await import('../../crates/node/web/js/block_prover_host.js');
function bridgeFixture() {
  const timers = new Map();
  let serial = 0;
  class Worker extends EventTarget {
    constructor(url, options) { super(); this.options = options; this.terminated = false; }
    terminate() { this.terminated = true; }
  }
  const target = { Worker,
    setTimeout(callback, ms) { const id = ++serial; timers.set(id, { callback, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  const guard = new BridgeStartup(target);
  const fire = ms => [...timers.values()].find(timer => timer.ms === ms).callback();
  return { guard, target, timers, Worker, fire };
}

for (const [name, type, message, expected] of [
  ['worker error', 'error', 'injected bridge worker error', /initialization failed: injected bridge worker error/],
  ['message error', 'messageerror', '', /bridge worker message error/],
  ['rejected initialization', 'error', 'injected bridge init rejection', /initialization failed: injected bridge init rejection/],
]) {
  test(`bridge startup reports ${name} before any host exists`, async () => {
    const { guard, target, timers, Worker } = bridgeFixture();
    assert.equal(timers.size, 2); // installed before the worker is constructed
    const other = new target.Worker('other', { name: 'another_worker' });
    const worker = new target.Worker('bridge', { name: 'mina_block_prover' });
    const event = new Event(type, { cancelable: true });
    Object.defineProperty(event, 'message', { value: message });
    worker.dispatchEvent(event);
    await assert.rejects(guard.failure, expected);
    guard.finish(false);
    assert.equal(worker.terminated, true);
    assert.equal(other.terminated, false);
    assert.equal(timers.size, 0);
    assert.equal(target.Worker, Worker);
  });
}

test('bridge initialization timeout is armed before a worker or first command', async () => {
  const { guard, target, timers, Worker, fire } = bridgeFixture();
  fire(60_000);
  await assert.rejects(guard.failure, /bridge initialization timed out/);
  guard.finish(false);
  const late = new target.Worker('late', { name: 'shim\nmina_block_prover' });
  assert.equal(late.terminated, true);
  assert.equal(target.Worker, Worker);
  assert.equal(timers.size, 0);
});

test('the overall startup deadline survives the first command', async () => {
  const { guard, target, timers, fire } = bridgeFixture();
  const worker = new target.Worker('bridge', { name: 'mina_block_prover' });
  guard.command();
  assert.equal(timers.size, 1);
  fire(1_350_000);
  await assert.rejects(guard.failure, /prover startup timed out/);
  guard.finish(false);
  assert.equal(worker.terminated, true);
});

test('successful readiness clears deadlines without stopping the bridge', async () => {
  const { guard, target, timers, Worker } = bridgeFixture();
  const worker = new target.Worker('bridge', { name: 'mina_block_prover' });
  guard.command();
  guard.finish(true);
  await guard.failure;
  assert.equal(worker.terminated, false);
  assert.equal(timers.size, 0);
  assert.equal(target.Worker, Worker);
});

test('an initialization error wins a race with successful readiness', async () => {
  const { guard, target, timers } = bridgeFixture();
  const worker = new target.Worker('bridge', { name: 'mina_block_prover' });
  guard.fail('initialization failed before completion');
  assert.equal(guard.finish(true), false);
  await assert.rejects(guard.failure, /initialization failed before completion/);
  assert.equal(worker.terminated, true);
  assert.equal(timers.size, 0);
});

test('queued commands cannot recreate a host after startup failure', async () => {
  const { beginBridgeStartup, bridgeStartupFailure, finishBridgeStartup, startBlockProverHost } =
    await import('../../crates/node/web/js/block_prover_host.js');
  const prior = globalThis.Worker;
  globalThis.Worker = class extends EventTarget { terminate() {} };
  try {
    beginBridgeStartup();
    const worker = new globalThis.Worker('bridge', { name: 'mina_block_prover' });
    const event = new Event('error', { cancelable: true });
    Object.defineProperty(event, 'message', { value: 'bridge init failed' });
    worker.dispatchEvent(event);
    await assert.rejects(bridgeStartupFailure(), /bridge init failed/);
    await finishBridgeStartup(false);
    // No DOM exists here: this must fail before trying to make an iframe.
    await assert.rejects(startBlockProverHost('glue', 2, 'devnet', 1, null, false, true, () => {}), /bridge init failed/);
  } finally {
    globalThis.Worker = prior;
  }
});
