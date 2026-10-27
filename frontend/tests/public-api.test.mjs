import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import vm from 'node:vm';
import ts from 'typescript';
import { of, firstValueFrom } from 'rxjs';
import {
  PUBLIC_FEATURES,
  PUBLIC_ROUTES,
  publicNodeDetails,
} from '../src/app/shared/helpers/public-api.ts';
import { freshness } from '../src/app/shared/helpers/freshness.helper.ts';
import { initialNode } from '../src/app/shared/helpers/node-selection.helper.ts';

const require = createRequire(import.meta.url);
// Load the actual Angular services/routes with inert framework decorators. HTTP
// calls and route registration are exercised without starting a browser or node.
function load(relative, mocks) {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  const code = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
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
const angular = {
  Injectable: () => value => value,
  NgModule: () => value => value,
};
const status = {
  chain_id: 'ab'.repeat(32),
  transition_frontier: {
    best_tip: { hash: 'block', height: 577755, timestamp: 1_000_000_000 },
    sync: { phase: 'Synced', time: 2_000_000_000 },
  },
};

test('public status maps tip and sync clocks without inventing operator measurements', () => {
  const details = publicNodeDetails(status, 3000);
  assert.equal(details.bestTipTimestamp, 1000);
  assert.equal(details.syncTime, 2000);
  assert.equal(details.sampledAt, 3000);
  assert.equal(details.chainId, status.chain_id);
  assert.equal(details.peersConnected, null);
  assert.equal(details.snarks, null);
  assert.equal(details.transactions, null);
  assert.equal(details.maxPeerHeight, null);
  assert.equal(freshness(details, 3000).synced, true);
  const starting = publicNodeDetails(
    {
      chain_id: null,
      transition_frontier: {
        best_tip: null,
        sync: { phase: 'Bootstrap', time: null },
      },
    },
    3000,
  );
  assert.equal(starting.bestTipTimestamp, null);
  assert.equal(freshness(starting, 3000).synced, false);
  assert.equal(freshness(details, 40_000).sampleStale, true);
});

function appService(publicApi) {
  return load('../src/app/app.service.ts', {
    '@angular/core': angular,
    '@shared/constants/config': {
      CONFIG: { publicApi, configs: [{ name: 'Public node', url: '/api' }] },
    },
    '@shared/helpers/public-api': { publicNodeDetails },
    '@shared/helpers/node-selection.helper': { initialNode },
    '@shared/helpers/mina.helper': { getNetwork: () => 'test' },
    '@shared/helpers/freshness.helper': {
      nanosToMs: x => (x == null ? null : x / 1_000_000),
    },
    '@shared/types/app/app-node-details.type': {
      AppNodeStatus: { PENDING: 'Pending', SYNCED: 'Synced' },
    },
    '@mina-rust/shared': {
      getLocalStorage: () => ({
        getItem: () => '[{"name":"stale private node"}]',
      }),
      ONE_MILLION: 1_000_000,
      nanOrElse: (x, fallback) => (Number.isNaN(x) ? fallback : x),
    },
  }).AppService;
}

test('public app polls only projected status, skips build_env and saved private nodes', async () => {
  const calls = [];
  const Service = appService(true);
  const service = new Service(
    {
      get: path => {
        calls.push(path);
        return of(status);
      },
    },
    {},
  );
  assert.equal(await firstValueFrom(service.getEnvBuild()), undefined);
  const details = await firstValueFrom(service.getActiveNodeDetails());
  assert.equal(details.blockHeight, 577755);
  assert.equal(details.status, 'Synced');
  const nodes = await firstValueFrom(service.getNodes());
  assert.equal(nodes.length, 1);
  assert.equal(nodes[0].url, '/api');
  assert.deepEqual(calls, ['/status']);
});

test('private app still requests build_env and full status', async () => {
  const calls = [];
  const full = {
    ...status,
    peers: [],
    transaction_pool: { transactions: 7 },
    snark_pool: { snarks: 2 },
  };
  const Service = appService(false);
  const service = new Service(
    {
      get: path => {
        calls.push(path);
        return of(path === '/status' ? full : { git: {} });
      },
    },
    {},
  );
  await firstValueFrom(service.getEnvBuild());
  const details = await firstValueFrom(service.getActiveNodeDetails());
  assert.equal(details.transactions, 7);
  assert.deepEqual(calls, ['/build_env', '/status']);
});

test('public route table excludes direct links to every operator screen', () => {
  function routes(publicApi) {
    return load('../src/app/app.routing.ts', {
      '@angular/core': angular,
      '@angular/router': { RouterModule: { forRoot: () => ({}) } },
      '@shared/constants/config': {
        CONFIG: { publicApi },
        getFirstFeature: () => 'blocks',
      },
      '@shared/helpers/public-api': { PUBLIC_ROUTES },
      '@app/layout/web-node-landing-page/web-node-landing-page.component': {},
      '@shared/guards/landing-page.guard': {},
      '@shared/guards/controls.guard': { controlsGuard: () => () => true },
    }).generateRoutes();
  }
  const publicRoutes = routes(true);
  assert.deepEqual(
    Array.from(publicRoutes, route => route.path),
    [...PUBLIC_ROUTES, '**'],
  );
  assert.equal(publicRoutes.at(-1).redirectTo, 'blocks');
  const privateRoutes = routes(false);
  for (const path of [
    'dashboard',
    'nodes',
    'resources',
    'network',
    'state',
    'snarks',
    'block-production',
    'mempool',
    'benchmarks',
    'fuzzing',
    'loading-web-node',
  ]) {
    assert.ok(privateRoutes.some(route => route.path === path));
    assert.ok(!publicRoutes.some(route => route.path === path));
  }
  assert.deepEqual(Object.keys(PUBLIC_FEATURES), ['blocks', 'accounts']);
});
