import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  WasmIntegrityError,
  fetchPinnedWasm,
  parsePinnedWasm,
  sha256Hex,
} from '../src/app/shared/helpers/wasm-integrity.helper.ts';

const bytes = new TextEncoder().encode('\0asm the web node').buffer;
// sha256sum of the same bytes, computed outside WebCrypto.
const { createHash } = await import('node:crypto');
const digest = createHash('sha256').update(Buffer.from(bytes)).digest('hex');
const url = 'https://assets.minaweb.org/wasm/mina_node_web_bg.0123.wasm';

function serve(body, status = 200) {
  return () =>
    Promise.resolve({
      ok: status >= 200 && status < 300,
      status,
      arrayBuffer: () => Promise.resolve(body),
    });
}

test('WebCrypto agrees with sha256sum', async () => {
  assert.equal(await sha256Hex(bytes), digest);
});

test('the pinned bytes are returned', async () => {
  const pin = parsePinnedWasm(url, digest.toUpperCase());
  assert.deepEqual(pin, { url, sha256: digest });
  const got = await fetchPinnedWasm(pin, serve(bytes));
  assert.equal(got, bytes);
});

test('other bytes are refused, naming both digests', async () => {
  const tampered = new Uint8Array(bytes.slice(0));
  tampered[tampered.length - 1] ^= 1;
  const pin = parsePinnedWasm(url, digest);
  await assert.rejects(fetchPinnedWasm(pin, serve(tampered.buffer)), error => {
    assert.ok(error instanceof WasmIntegrityError);
    assert.match(error.message, /not the one this page was built with/);
    assert.match(error.message, new RegExp(`expected ${digest}`));
    assert.match(error.message, /not starting the node/);
    return true;
  });
});

test('an HTTP error is refused', async () => {
  await assert.rejects(
    fetchPinnedWasm(parsePinnedWasm(url, digest), serve(bytes, 404)),
    /HTTP 404/,
  );
});

test('no pin is the same-origin build; half a pin refuses', () => {
  assert.equal(parsePinnedWasm('', ''), null);
  assert.equal(parsePinnedWasm(undefined, undefined), null);
  for (const [u, d] of [
    [url, ''],
    [url, 'abc'],
    [url, 'z'.repeat(64)],
    ['', digest],
  ]) {
    assert.throws(() => parsePinnedWasm(u, d), WasmIntegrityError);
  }
});
