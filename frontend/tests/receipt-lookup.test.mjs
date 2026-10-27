import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  lookupResult,
  needsAnotherPage,
  pagesResult,
  RECEIPT_PAGE_SIZE,
} from '../src/app/features/verification/verification.types.ts';
import {
  ALL_CONTROLS,
  parseControls,
  VIEW_ONLY,
} from '../src/app/shared/constants/build.ts';

// R26a: a block's receipt is found by its state hash wherever it sits in
// the node's 1,024 retained receipts. Telemetry contract, not crypto vectors.
const receipt = hash => ({
  block: { stateHash: hash, height: 1 },
  blockProof: { status: 'passed' },
});
// A node's pages, newest first: page i holds receipts h<i*128> onwards.
const page = (index, count = RECEIPT_PAGE_SIZE, capacity = 1024) => ({
  capacity,
  receipts: Array.from({ length: count }, (_, i) =>
    receipt(`h${index * RECEIPT_PAGE_SIZE + i}`),
  ),
});

test('a node that looks hashes up answers in one request', () => {
  const result = lookupResult({
    capacity: 1024,
    receipts: [receipt('tip')],
    lookup: { requested: 2, found: 1 },
  });
  assert.equal(result.method, 'hash');
  assert.equal(result.complete, true);
  assert.equal(result.requests, 1);
  assert.equal(result.receipts.get('tip').blockProof.status, 'passed');
  assert.equal(result.receipts.has('other'), false);
});

test('an older node is paged until the tip pushed off page one is found', () => {
  // R26: the verified tip sat at offset 256 after bootstrap.
  const hashes = ['h1', 'h290'];
  const pages = [page(0)];
  assert.equal(needsAnotherPage(hashes, pages), true);
  pages.push(page(1));
  assert.equal(needsAnotherPage(hashes, pages), true);
  pages.push(page(2));
  assert.equal(needsAnotherPage(hashes, pages), false);
  const result = pagesResult(hashes, pages);
  assert.equal(result.method, 'pages');
  assert.equal(result.requests, 3);
  assert.equal(result.complete, true);
  assert.deepEqual([...result.receipts.keys()].sort(), ['h1', 'h290']);
});

test('paging is bounded by the capacity and stops at a short page', () => {
  const missing = ['never-seen'];
  const full = Array.from({ length: 8 }, (_, i) => page(i));
  assert.equal(needsAnotherPage(missing, full.slice(0, 7)), true);
  assert.equal(needsAnotherPage(missing, full), false);
  // Every retained receipt read: missing means none retained.
  assert.equal(pagesResult(missing, full).complete, true);
  const short = [page(0), page(1, 5)];
  assert.equal(needsAnotherPage(missing, short), false);
  assert.equal(pagesResult(missing, short).complete, true);
  assert.equal(pagesResult(missing, short).receipts.size, 0);
});

test('paging stops at 1,024 receipts whatever capacity the node reports', () => {
  // Review of R26a: the request bound must not trust the node's answer.
  const missing = ['never-seen'];
  const boastful = Array.from({ length: 8 }, (_, i) => page(i, 128, 1e9));
  assert.equal(needsAnotherPage(missing, boastful.slice(0, 7)), true);
  assert.equal(needsAnotherPage(missing, boastful), false);
  assert.equal(pagesResult(missing, boastful).complete, true);
  const unknown = Array.from({ length: 8 }, (_, i) => page(i, 128, undefined));
  assert.equal(needsAnotherPage(missing, unknown), false);
  // A smaller capacity still stops paging earlier.
  assert.equal(
    needsAnotherPage(missing, [page(0, 128, 256), page(1, 128, 256)]),
    false,
  );
});

test('view-only unless controls are named', () => {
  assert.deepEqual(parseControls(''), VIEW_ONLY);
  assert.deepEqual(parseControls(undefined), VIEW_ONLY);
  assert.deepEqual(parseControls('all'), ALL_CONTROLS);
  assert.deepEqual(parseControls(' Producer '), {
    producer: true,
    send: false,
    snarker: false,
  });
  assert.deepEqual(parseControls('send,typo'), {
    producer: false,
    send: true,
    snarker: false,
  });
  assert.deepEqual(parseControls('snarker'), {
    producer: false,
    send: false,
    snarker: true,
  });
});
