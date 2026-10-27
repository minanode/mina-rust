import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  initialNode,
  needsWebNodeStart,
  pageOf,
} from '../src/app/shared/helpers/node-selection.helper.ts';

// The demo build's two nodes: the browser node first, as configured.
const web = {
  name: 'Web Node',
  isWebNode: true,
  pages: ['dashboard', 'state', 'mempool'],
};
const native = {
  name: 'Native mainnet node',
  pages: ['dashboard', 'blocks', 'accounts', 'mempool', 'nodes', 'network'],
};
const nodes = [web, native];
const supports = (node, page) => node.pages.includes(page);
const base = 'http://127.0.0.1:4288';
const hash = '3NKammKoyyUMP9VZsVk8Wmts9B4v3yfM1VqyftSWGDgsnn4ZM5WP';

test('reads the page from a URL', () => {
  assert.equal(pageOf(`${base}/blocks/${hash}?node=x`), 'blocks');
  assert.equal(pageOf(`${base}/`), '');
});

test('a reloaded native block page stays on the native node', () => {
  const node = initialNode(
    nodes,
    `${base}/blocks/${hash}?node=Native%20mainnet%20node`,
    supports,
  );
  assert.equal(node, native);
  assert.equal(needsWebNodeStart(node, false), false);
});

test('a block link without a node parameter picks a node that has blocks', () => {
  assert.equal(initialNode(nodes, `${base}/blocks/${hash}`, supports), native);
  assert.equal(
    initialNode(nodes, `${base}/accounts?publicKey=B62q`, supports),
    native,
  );
});

test('the entry page and web node pages still start on the web node', () => {
  for (const path of [
    '/',
    '/dashboard',
    '/loading-web-node',
    '/dashboard?node=Web%20Node',
  ]) {
    const node = initialNode(nodes, `${base}${path}`, supports);
    assert.equal(node, web, path);
    assert.equal(needsWebNodeStart(node, false), true, path);
    assert.equal(needsWebNodeStart(node, true), false, path);
  }
});

test('an unknown node name falls back as if there were none', () => {
  assert.equal(
    initialNode(nodes, `${base}/blocks?node=Gone`, supports),
    native,
  );
  assert.equal(initialNode(nodes, `${base}/state?node=Gone`, supports), web);
});

test('a page no node has falls back to the first node', () => {
  assert.equal(initialNode(nodes, `${base}/fuzzing`, supports), web);
  assert.equal(initialNode([], `${base}/blocks`, supports), undefined);
});
