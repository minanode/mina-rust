import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { test } from 'node:test';

const src = new URL('../src/', import.meta.url);
const template = await readFile(
  new URL('app/shared/components/brand-logo/brand-logo.component.html', src),
  'utf8',
);
const component = await readFile(
  new URL('app/shared/components/brand-logo/brand-logo.component.ts', src),
  'utf8',
);
const numbers = d => d.match(/-?\d*\.?\d+/g).map(Number);

// The brand's 48 x 48 master: ui/minanode-site/app/icon.svg.
const MASTER = 'M6 36V22C6 9 24 9 24 22V36M24 22C24 9 42 9 42 22V36';

test('the mark is the twin-arches master scaled to 32 px', () => {
  const d = template.match(/class="mark"\s+d="([^"]+)"/)[1];
  const scaled = numbers(MASTER).map(n => (n * 32) / 48);
  numbers(d).forEach((n, i) => assert.ok(Math.abs(n - scaled[i]) < 1e-3));
  const stroke = Number(template.match(/stroke-width="([^"]+)"/)[1]);
  assert.ok(Math.abs(stroke - (5.5 * 32) / 48) < 1e-3);
});

test('the wordmark fits the view box, 11 px after the mark', () => {
  const width = Number(component.match(/const WIDTH = (\d+);/)[1]);
  assert.match(template, /<g transform="translate\(43 0\) scale\(0\.1\)">/);
  for (const name of ['mina', 'node']) {
    const d = template.match(new RegExp(`class="${name}"\\s+d="([^"]+)"`))[1];
    const xy = numbers(d);
    const xs = xy.filter((_, i) => i % 2 === 0).map(x => 43 + x / 10);
    const ys = xy.filter((_, i) => i % 2 === 1).map(y => y / 10);
    assert.ok(Math.min(...xs) >= 43 && Math.max(...xs) <= width, name);
    assert.ok(Math.min(...ys) >= 0 && Math.max(...ys) <= 32, name);
  }
});

test('no template still uses the old logo files', async () => {
  const old = /logo-text\.svg|text-logo-text\.svg|web-node-demo\/Logo\.svg/;
  const walk = async dir => {
    for (const e of await readdir(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) await walk(p);
      else if (/\.(html|ts|scss)$/.test(e.name)) {
        assert.doesNotMatch(await readFile(p, 'utf8'), old, p);
      }
    }
  };
  await walk(new URL('app/', src).pathname);
});
