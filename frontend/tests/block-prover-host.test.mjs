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
