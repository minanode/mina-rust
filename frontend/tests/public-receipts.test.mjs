import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import vm from 'node:vm';
import ts from 'typescript';
import { firstValueFrom, filter, of } from 'rxjs';
import * as types from '../src/app/features/verification/verification.types.ts';
import * as poll from '../src/app/features/verification/verification-poll.ts';

const require = createRequire(import.meta.url);
// Load the actual service with inert Angular decorators and injected fakes,
// as public-api.test.mjs does, so its HTTP calls run without a browser.
function load(relative, mocks) {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  const code = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      // The default ES5 target would turn `[...map.keys()]` into [].
      target: ts.ScriptTarget.ES2022,
      experimentalDecorators: true,
    },
  }).outputText;
  const exports = {};
  vm.runInNewContext(code, {
    exports,
    require: name => (Object.hasOwn(mocks, name) ? mocks[name] : require(name)),
    Date,
  });
  return exports;
}

const HASHES = Array.from({ length: 21 }, (_, i) => `3N${i}`);
const check = elapsedMs => ({
  status: 'passed',
  elapsedMs,
});
// A public node's answer: receipts for some hashes, the rest left out.
const PUBLIC_RECEIPT = {
  stateHash: HASHES[0],
  height: '564494',
  network: 'mainnet',
  arrival: 'gossip',
  applied: true,
  blockProof: check(1500.5),
  deltaChain: check(null),
  bodyReference: check(null),
  verifierKey: 'ab'.repeat(32),
};

function makeService(publicApi) {
  const calls = [];
  const HttpClient = {};
  const Store = {};
  const WebNodeService = {};
  const http = {
    post: (url, body) => {
      calls.push({ method: 'POST', url, body });
      const wanted = new Set(body.variables.stateHashes);
      return of({
        data: {
          verificationReceipts: wanted.has(PUBLIC_RECEIPT.stateHash)
            ? [PUBLIC_RECEIPT]
            : [],
        },
      });
    },
    get: (url, options) => {
      calls.push({ method: 'GET', url, options });
      return of({
        capacity: 1024,
        receipts: [],
        lookup: { requested: 0, found: 0 },
      });
    },
  };
  const injected = new Map([
    [HttpClient, http],
    [WebNodeService, {}],
    [Store, { select: () => of({ url: '/api', isWebNode: false }) }],
  ]);
  const Service = load(
    '../src/app/features/verification/verification.service.ts',
    {
      '@angular/core': {
        Injectable: () => value => value,
        inject: token => injected.get(token),
      },
      '@angular/common/http': {
        HttpClient,
        HttpParams: class {
          constructor(values = {}) {
            this.values = values;
          }
          set(key, value) {
            return new this.constructor({ ...this.values, [key]: value });
          }
        },
      },
      '@ngrx/store': { Store },
      '@app/app.state': { AppSelectors: { activeNode: 'activeNode' } },
      '@app/app.setup': {},
      '@core/services/web-node.service': { WebNodeService },
      '@shared/types/core/environment/mina-env.type': {},
      '@shared/constants/config': { CONFIG: { publicApi } },
      './verification.types': types,
      './verification-poll': poll,
    },
  ).VerificationService;
  return { service: new Service(), calls };
}

/** Every hash's receipt view, once each has been looked up. */
async function views(service) {
  const subscriptions = [];
  const answered = HASHES.map(
    hash =>
      new Promise(resolve =>
        subscriptions.push(
          service
            .receipt$(hash)
            .pipe(filter(view => !view.loading))
            .subscribe(resolve),
        ),
      ),
  );
  const result = await Promise.all(answered);
  subscriptions.forEach(s => s.unsubscribe());
  return result;
}

test('public preset looks receipts up through allow-listed GraphQL, 20 hashes a request', async () => {
  const { service, calls } = makeService(true);
  const result = await views(service);
  assert.ok(calls.length >= 2);
  for (const call of calls) {
    assert.equal(call.method, 'POST');
    assert.equal(call.url, '/api/graphql');
    assert.equal(call.body.query, types.PUBLIC_RECEIPTS_QUERY);
    assert.ok(call.body.variables.stateHashes.length <= 20);
  }
  // 21 hashes: one full page of 20, then the last one.
  assert.deepEqual(
    calls.slice(0, 2).map(c => c.body.variables.stateHashes.length),
    [20, 1],
  );
  const [found, missing] = result;
  assert.equal(found.error, false);
  assert.equal(found.receipt.block.stateHash, HASHES[0]);
  assert.equal(found.receipt.block.height, 564494);
  assert.equal(found.receipt.blockProof.checkedAt, null);
  assert.equal(found.receipt.deltaChain.checkedAt, null);
  assert.equal(found.receipt.bodyReference.checkedAt, null);
  assert.equal(types.receiptLabel(found.receipt), 'Proof verified here');
  // The node searched every retained receipt: none means none retained.
  assert.equal(missing.receipt, undefined);
  assert.equal(missing.complete, true);
  assert.equal(types.receiptLabel(missing.receipt), types.NO_RETAINED_RECEIPT);
});

test('private presets keep the operator receipt route', async () => {
  const { service, calls } = makeService(false);
  await views(service);
  assert.ok(calls.length >= 1);
  for (const call of calls) {
    assert.equal(call.method, 'GET');
    assert.equal(call.url, '/api/stats/verification');
    assert.equal(call.options.params.values.limit, types.RECEIPT_PAGE_SIZE);
  }
  assert.equal(calls[0].options.params.values.hashes, HASHES.join(','));
});

test('a public receipt maps to the receipt the components show', () => {
  const receipt = types.publicReceipt({
    ...PUBLIC_RECEIPT,
    blockProof: { status: 'pending', elapsedMs: null },
    arrival: 'bootstrap',
  });
  assert.deepEqual(receipt.block, { stateHash: HASHES[0], height: 564494 });
  assert.equal(receipt.blockProof.checkedAt, null);
  assert.equal(receipt.receivedAt, undefined);
  assert.equal(types.receiptLabel(receipt), 'Proof verification pending');
  assert.equal(types.checkLabel(receipt.deltaChain), 'Passed');
  // The query asks for exactly the public fields the mapping reads.
  for (const field of Object.keys(PUBLIC_RECEIPT)) {
    assert.match(types.PUBLIC_RECEIPTS_QUERY, new RegExp(`\\b${field}\\b`));
  }
  assert.doesNotMatch(
    types.PUBLIC_RECEIPTS_QUERY,
    /receivedAt|checkedAt|capacity/,
  );
});

test('blocks and block detail show receipts in every preset', () => {
  const read = name =>
    readFileSync(
      new URL(`../src/app/features/chain/${name}`, import.meta.url),
      'utf8',
    );
  const blocks = read('blocks.component.html');
  const detail = read('block-detail.component.html');
  // The only public-only condition left hides the operator counters panel.
  assert.deepEqual(blocks.match(/@if \(!publicApi\) \{[^}]*\}/g), [
    '@if (!publicApi) {\n  <mina-verification-panel />\n}',
  ]);
  assert.doesNotMatch(detail, /publicApi/);
  assert.match(blocks, /<th>Verification<\/th>/);
  assert.match(blocks, /<mina-verification-receipt \[stateHash\]/);
  assert.match(detail, /<mina-verification-receipt/);
});
