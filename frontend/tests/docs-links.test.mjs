import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DOCS_BASE_URL,
  docsUrl,
  docsUrlForPath,
} from '../src/app/shared/constants/docs.ts';

test('every docs link builds on the one base URL', () => {
  assert.equal(DOCS_BASE_URL, 'https://minanode.org/docs/');
  assert.equal(docsUrl(), DOCS_BASE_URL);
  assert.equal(docsUrl('dashboard'), 'https://minanode.org/docs/dashboard/');
});

test('pages with a matching docs page link to it', () => {
  assert.equal(
    docsUrlForPath('/dashboard'),
    'https://minanode.org/docs/dashboard/',
  );
  assert.equal(
    docsUrlForPath('/block-production/overview/8'),
    'https://minanode.org/docs/block-producer/',
  );
  assert.equal(
    docsUrlForPath('/block-production/won-slots'),
    'https://minanode.org/docs/block-producer/',
  );
  assert.equal(
    docsUrlForPath('/snarks/worker'),
    'https://minanode.org/docs/snark-worker/',
  );
});

test('other pages have no contextual docs link', () => {
  for (const path of [
    '/',
    '/blocks',
    '/snarks/work-pool',
    '/snarks/scan-state',
    '/dashboards',
    '/block-productions',
  ]) {
    assert.equal(docsUrlForPath(path), undefined, path);
  }
});
