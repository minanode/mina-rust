import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import ts from 'typescript';
import * as rx from 'rxjs';
import '@angular/compiler';
import * as ngrx from '@ngrx/store';
import * as effects from '@ngrx/effects';

// Execute the production TypeScript effects, services and reducers in Node.
// Only browser/DI boundaries are stubbed; RxJS and NgRx are the real packages.
const require = createRequire(import.meta.url);
class BaseEffect {
  constructor(store) {
    this.store = store;
  }
  latestActionState() {
    return rx.pipe(
      rx.withLatestFrom(this.store.state$),
      rx.map(([action, state]) => ({ action, state })),
    );
  }
}
const shared = {
  getLocalStorage: () => undefined,
  isMobile: () => false,
  sort: values => values,
  SortDirection: { DSC: 'DSC' },
  removeParamsFromURL: url => url,
};
const stubs = {
  '@angular/core': {
    Injectable: () => value => value,
    Component: () => value => value,
    ViewChild: () => () => {},
    ChangeDetectionStrategy: { OnPush: 0 },
  },
  d3: {},
  'd3-zoom': {},
  mathjs: { eigs: () => ({ values: [0, 1] }) },
  '@network/splits/dashboard-splits.state': {
    selectDashboardSplitsPeersAndLinksAndSetsAndFetching: 'graph',
  },
  '@shared/base-classes/store-dispatcher.class': {
    StoreDispatcher: class {
      select(selector, callback, ...operators) {
        this.subscription = this.state$.pipe(...operators).subscribe(callback);
      }
    },
  },
  '@ngrx/store': ngrx,
  '@ngrx/effects': effects,
  '@mina-rust/shared': shared,
  '@app/app.setup': { selectMinaState: state => state },
  '@shared/base-classes/mina-rust-base.effect': {
    BaseEffect,
    MinaRustBaseEffect: BaseEffect,
  },
  '@shared/constants/store-functions': {
    createType: (prefix, sub, type) => `[${prefix}] ${type}`,
    addError: error => ({ type: 'ERROR', error }),
  },
  '@shared/constants/config': { isFeatureEnabled: () => true },
};
const cache = new Map();
function load(path) {
  if (cache.has(path)) return cache.get(path);
  const filename = fileURLToPath(
    new URL(`../src/app/${path}.ts`, import.meta.url),
  );
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
      experimentalDecorators: true,
    },
  });
  const module = { exports: {} };
  cache.set(path, module.exports);
  const resolve = name => {
    if (name in stubs) return stubs[name];
    for (const [prefix, root] of [
      ['@app/', ''],
      ['@shared/', 'shared/'],
      ['@network/', 'features/network/'],
    ]) {
      if (name.startsWith(prefix))
        return load(root + name.slice(prefix.length));
    }
    return require(name);
  };
  new Function('require', 'module', 'exports', outputText)(
    resolve,
    module,
    module.exports,
  );
  return module.exports;
}
const { AppActions } = load('app.actions');
const { appReducer } = load('app.reducer');
const { AppEffects } = load('app.effects');
const { DashboardSplitsService } = load(
  'features/network/splits/dashboard-splits.service',
);
const { DashboardSplitsEffects } = load(
  'features/network/splits/dashboard-splits.effects',
);
const { topologyReducer } = load(
  'features/network/splits/dashboard-splits.reducer',
);
const { DashboardSplitsGetSplits, DashboardSplitsClose } = load(
  'features/network/splits/dashboard-splits.actions',
);
const { AppNodeStatus } = load('shared/types/app/app-node-details.type');
const A = { name: 'A', url: 'http://a.invalid' };
const B = { name: 'B', url: 'http://b.invalid' };
const status = (height, network, sampledAt) => ({
  status: AppNodeStatus.SYNCED,
  blockHeight: height,
  network,
  sampledAt,
});
const peer = {
  peer_id: 'peer-a',
  address: '/ip4/8.210.56.69/tcp/8302',
  connection_status: 'Connected',
};

function harness(t) {
  const actions$ = new rx.Subject();
  const state$ = new rx.BehaviorSubject({
    app: appReducer(
      undefined,
      AppActions.initSuccess({ activeNode: A, nodes: [A, B] }),
    ),
    topology: topologyReducer(undefined, { type: 'INIT' }),
  });
  const dispatch = action => {
    const state = state$.value;
    state$.next({
      app: appReducer(state.app, action),
      topology: topologyReducer(state.topology, action),
    });
    actions$.next(action);
  };
  const requests = [];
  const rust = {
    name: 'A',
    changeRustNode(node) {
      this.name = node.name;
    },
    get(path, node) {
      const response = new rx.Subject();
      requests.push({ node: node ?? { name: this.name }, path, response });
      return response;
    },
  };
  const store = { state$ };
  const app = new AppEffects(
    actions$,
    { getActiveNodeDetails: node => rust.get('/status', node) },
    rust,
    { url: '/network/splits', navigate() {} },
    { started: true },
    store,
  );
  const topology = new DashboardSplitsEffects(
    actions$,
    new DashboardSplitsService(rust),
    store,
  );
  const subscriptions = [
    app.onNodeChange$,
    app.getNodeDetails$,
    topology.getSplits$,
  ].map(stream => stream.subscribe(dispatch));
  t.after(() => subscriptions.forEach(s => s.unsubscribe()));
  return { dispatch, state$, requests, rust };
}

test('topology clears A on selection and refetches B; slow A cannot fill B', t => {
  const { dispatch, state$, requests } = harness(t);
  dispatch(new DashboardSplitsGetSplits());
  requests[0].response.next([peer]);
  assert.equal(state$.value.topology.peers.length, 2);
  dispatch(new DashboardSplitsGetSplits());
  const slowA = requests.at(-1);
  dispatch(AppActions.changeActiveNode({ node: B }));
  assert.deepEqual(state$.value.topology.peers, []);
  assert.deepEqual(state$.value.topology.links, []);
  assert.deepEqual(state$.value.topology.sets, []);
  assert.equal(state$.value.topology.activePeer, undefined);
  const b = requests.find(
    r => r.node?.name === 'B' && r.path === '/state/peers',
  );
  assert.ok(b, 'B refetch starts without clicking Refetch');
  slowA.response.next([peer]);
  assert.deepEqual(state$.value.topology.peers, []);
  b.response.next([]);
  assert.equal(state$.value.topology.node, 'B');
  assert.equal(state$.value.topology.peers[0].address, 'B');
  slowA.response.next([peer]);
  assert.equal(state$.value.topology.peers.length, 1);
  dispatch(new DashboardSplitsClose());
  dispatch(AppActions.changeActiveNode({ node: A }));
  assert.equal(
    requests.filter(r => r.path === '/state/peers').length,
    3,
    'closed topology does not refetch',
  );
});

test('topology service labels the answer with its request node', () => {
  const response = new rx.Subject();
  const rust = { name: 'A', get: () => response };
  let answer;
  new DashboardSplitsService(rust).getPeers(A).subscribe(value => {
    answer = value;
  });
  rust.name = 'B';
  response.next([peer]);
  assert.equal(answer.peers[0].address, 'A');
  assert.equal(answer.node, 'A');
});

test('status refresh for B is not suppressed by slow A and cannot inherit its identity', t => {
  const { dispatch, state$, requests } = harness(t);
  dispatch(AppActions.getNodeDetails());
  const slowA = requests[0];
  dispatch(AppActions.getNodeDetails());
  assert.equal(requests.length, 1, 'same-node polls do not overlap');
  dispatch(AppActions.changeActiveNode({ node: B }));
  const b = requests.find(r => r.node?.name === 'B' && r.path === '/status');
  assert.ok(b, 'B status request starts while A is outstanding');
  slowA.response.next(status(564795, 'mainnet', 1000));
  assert.equal(state$.value.app.activeNodeDetails.blockHeight, null);
  assert.equal(state$.value.app.activeNodeDetails.network, undefined);
  assert.equal(state$.value.app.activeNodeDetails.sampledAt, undefined);
  b.response.next(status(123, 'devnet', 2000));
  b.response.complete();
  slowA.response.next(status(564795, 'mainnet', 3000));
  assert.deepEqual(
    state$.value.app.activeNodeDetails,
    status(123, 'devnet', 2000),
  );
});

test('a late status failure from A cannot mark B offline; B failures retain only B freshness', t => {
  const { dispatch, state$, requests } = harness(t);
  dispatch(AppActions.getNodeDetails());
  const slowA = requests[0];
  dispatch(AppActions.changeActiveNode({ node: B }));
  const b = requests.find(r => r.node?.name === 'B');
  assert.ok(b, 'B refresh must start');
  b.response.next(status(123, 'devnet', 2000));
  b.response.complete();
  slowA.response.error(new Error('A offline'));
  assert.equal(state$.value.app.activeNodeDetails.status, AppNodeStatus.SYNCED);
  dispatch(AppActions.getNodeDetails());
  requests.at(-1).response.error(new Error('B offline'));
  const details = state$.value.app.activeNodeDetails;
  assert.equal(details.status, AppNodeStatus.OFFLINE);
  assert.equal(details.blockHeight, 123);
  assert.equal(details.network, 'devnet');
  assert.equal(details.sampledAt, 2000);
  dispatch(AppActions.getNodeDetails());
  assert.equal(requests.length, 4, 'polling resumes after an error');
});

test('reducers reject already queued answers from another node', t => {
  const { dispatch, state$ } = harness(t);
  dispatch(AppActions.changeActiveNode({ node: B }));
  dispatch(
    AppActions.getNodeDetailsSuccess({
      node: 'A',
      details: status(564795, 'mainnet', 1000),
    }),
  );
  dispatch({
    type: 'DASHBOARD_SPLITS_GET_SPLITS_SUCCESS',
    payload: { node: 'A', peers: [{ address: 'A', self: true }], links: [] },
  });
  assert.equal(state$.value.app.activeNodeDetails.blockHeight, null);
  assert.deepEqual(state$.value.topology.peers, []);
});

test('rendered topology clears during a switch and an awaiting old render cannot restore A', async t => {
  t.mock.method(console, 'log', () => {});
  const { DashboardSplitsGraphComponent } = load(
    'features/network/splits/dashboard-splits-graph/dashboard-splits-graph.component',
  );
  const graph = new DashboardSplitsGraphComponent();
  let displayed = [];
  let stopped = 0;
  graph.svg = {
    selectAll: () => ({
      remove: () => {
        displayed = [];
      },
    }),
  };
  graph.tooltip = { style() {} };
  graph.simulation = {
    stop: () => {
      stopped++;
    },
  };
  graph.renderGraph = ({ peers }) => {
    displayed = peers;
  };
  const aPeers = [{ address: 'A', self: true }, { address: 'peer-a' }];
  graph.state$ = new rx.BehaviorSubject({
    peers: aPeers,
    links: [{ source: 'A', target: 'peer-a' }],
    sets: [],
    fetching: false,
  });
  graph.listenToPeersLinksSets();
  t.after(() => graph.subscription.unsubscribe());
  graph.state$.next({ peers: [], links: [], sets: [], fetching: true });
  assert.deepEqual(displayed, []);
  assert.equal(stopped, 1);
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(
    displayed,
    [],
    'A must remain cleared after its awaited diagnostics finish',
  );
  graph.state$.next({
    peers: [{ address: 'B', self: true }],
    links: [],
    sets: [],
    fetching: false,
  });
  assert.equal(
    displayed[0].address,
    'B',
    'a node with zero connections still renders its own identity',
  );
});
