import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import vm from 'node:vm';
import { test } from 'node:test';
import ts from 'typescript';
import { BehaviorSubject, EMPTY, NEVER, of, Subject, throwError } from 'rxjs';
import { TestScheduler } from 'rxjs/testing';
import * as types from '../src/app/features/verification/verification.types.ts';
import * as poll from '../src/app/features/verification/verification-poll.ts';
import { freshness } from '../src/app/shared/helpers/freshness.helper.ts';

const require = createRequire(import.meta.url);
// Exercise the real Angular classes with injected service/store/HTTP fakes.
function load(relative, mocks) {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  const code = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
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

// Synthetic telemetry, not proof/consensus vectors.
const receipt = {
  block: { stateHash: '3Nsynthetic-receipt-for-polling', height: 42 },
  network: 'devnet',
  arrival: 'gossip',
  applied: true,
  blockProof: { status: 'passed', checkedAt: 1000, elapsedMs: 12 },
  deltaChain: { status: 'passed', checkedAt: null, elapsedMs: null },
  bodyReference: { status: 'passed', checkedAt: null, elapsedMs: null },
  verifierKey: null,
};
const snapshot = {
  receipts: [receipt],
  capacity: 1024,
  verifiedSinceStart: 1,
  failuresSinceStart: 0,
  recentFailureCount: 0,
  recentFailures: [],
  medianProofTimeMs: 12,
  lastVerified: receipt,
  lookup: { requested: 1, found: 1 },
};

function setup({ web = false, answer = () => of(snapshot) } = {}) {
  const node = new BehaviorSubject({ url: '/node-a', isWebNode: web });
  const details = new BehaviorSubject({
    status: 'Synced',
    bestTipHash: 'tip-0',
    blockHeight: 42,
    bestTipTimestamp: Date.now(),
  });
  const Store = {},
    HttpClient = {},
    WebNodeService = {},
    ServiceToken = {};
  const store = { select: key => (key === 'node' ? node : details) };
  let calls = 0;
  const request = () => {
    calls++;
    return answer(calls);
  };
  const injection = new Map([
    [Store, store],
    [HttpClient, { get: request, post: request }],
    [WebNodeService, { verificationQuery$: request }],
  ]);
  const mocks = {
    '@angular/core': {
      Injectable: () => value => value,
      Component: () => value => value,
      inject: token => injection.get(token),
      ChangeDetectionStrategy: { OnPush: 'OnPush' },
    },
    '@angular/common': {},
    '@angular/common/http': {
      HttpClient,
      HttpParams: class {
        set() {
          return this;
        }
      },
    },
    '@ngrx/store': { Store },
    '@app/app.state': {
      AppSelectors: { activeNode: 'node', activeNodeDetails: 'details' },
    },
    '@app/app.setup': {},
    '@core/services/web-node.service': { WebNodeService },
    '@shared/types/core/environment/mina-env.type': {},
    '@shared/constants/config': { CONFIG: { publicApi: false } },
    '@shared/helpers/freshness.helper': { freshness },
    './verification.types': types,
    './verification-poll': poll,
    './verification-receipt.component': {},
  };
  const Service = load(
    '../src/app/features/verification/verification.service.ts',
    mocks,
  ).VerificationService;
  const service = new Service();
  injection.set(ServiceToken, service);
  mocks['./verification.service'] = { VerificationService: ServiceToken };
  const Panel = load(
    '../src/app/features/verification/verification-panel.component.ts',
    mocks,
  ).VerificationPanelComponent;
  return {
    service,
    panel: new Panel(),
    node,
    details,
    calls: () => calls,
    tip: n => details.next({ ...details.value, bestTipHash: `tip-${n}` }),
  };
}

for (const web of [false, true]) {
  test(`${web ? 'WASM' : 'native'} panel keeps last good data across failure, timeout and empty answers`, () => {
    const scheduler = new TestScheduler(assert.deepEqual);
    scheduler.run(() => {
      // First two refreshes can fire at subscription (tip + initial timer).
      let mode = 'success';
      const h = setup({
        web,
        answer: () =>
          ({
            success: of(snapshot),
            failure: throwError(() => new Error('busy')),
            timeout: NEVER,
            empty: of(undefined),
            complete: EMPTY,
            noReceipts: of({ ...snapshot, receipts: [] }),
          })[mode],
      });
      let state;
      const sub = h.panel.state$.subscribe(v => (state = v));
      const saved = {};
      scheduler.schedule(() => {
        saved.updatedAt = state.updatedAt;
        mode = 'failure';
        h.tip(1);
      }, 10);
      scheduler.schedule(() => {
        assert.equal(state.data, snapshot);
        assert.equal(state.unavailable, false);
        assert.equal(state.error, true);
        assert.equal(state.updatedAt, saved.updatedAt);
        mode = 'timeout';
        h.tip(2);
      }, 20);
      scheduler.schedule(() => {
        assert.equal(state.data, snapshot);
        assert.equal(state.unavailable, false);
        assert.equal(state.error, true);
        assert.equal(state.updatedAt, saved.updatedAt);
        mode = 'empty';
        h.tip(3);
      }, 20021);
      scheduler.schedule(() => {
        assert.equal(state.data, snapshot);
        assert.equal(state.unavailable, false);
        mode = 'complete';
        h.tip(4);
      }, 20022);
      scheduler.schedule(() => {
        assert.equal(state.data, snapshot);
        mode = 'noReceipts';
        h.tip(5);
      }, 20023);
      scheduler.schedule(() => {
        assert.equal(state.data, snapshot);
        assert.equal(state.updatedAt, saved.updatedAt);
        assert.equal(state.unavailable, false);
        mode = 'success';
        h.tip(6);
      }, 20024);
      scheduler.schedule(() => {
        assert.equal(state.error, false);
        sub.unsubscribe();
      }, 20025);
    });
  });
}

test('panel distinguishes never succeeded, bootstrap and unsupported', () => {
  const scheduler = new TestScheduler(assert.deepEqual);
  scheduler.run(() => {
    let unsupported = false;
    const h = setup({
      answer: () =>
        throwError(() => (unsupported ? { status: 404 } : new Error('busy'))),
    });
    let state;
    const sub = h.panel.state$.subscribe(v => (state = v));
    scheduler.schedule(() => {
      assert.equal(state.unavailable, true);
      h.details.next({ status: 'Bootstrap' });
      assert.equal(state.bootstrap, true);
      assert.equal(state.unavailable, false);
      unsupported = true;
      h.tip('unsupported');
      assert.equal(state.unsupported, true);
      assert.equal(state.data, null);
      sub.unsubscribe();
    }, 1);
  });
});

test('new best tips refresh, repeated status samples do not, and in-flight polls exhaust triggers', () => {
  new TestScheduler(assert.deepEqual).run(({ cold }) => {
    const h = setup({ answer: () => cold('-----a|', { a: snapshot }) });
    const sub = h.service.state$.subscribe();
    h.tip(1);
    h.tip(2);
    assert.equal(h.calls(), 1);
    sub.unsubscribe();
  });
  const scheduler = new TestScheduler(assert.deepEqual);
  scheduler.run(() => {
    const h = setup();
    const sub = h.service.state$.subscribe();
    scheduler.schedule(() => {
      const calls = h.calls();
      h.details.next({ ...h.details.value, sampledAt: Date.now() });
      assert.equal(h.calls(), calls);
      h.tip(3);
      assert.equal(h.calls(), calls + 1);
    }, 10);
    scheduler.schedule(() => {
      assert.equal(h.calls(), 3);
    }, 59999);
    scheduler.schedule(() => {
      assert.equal(h.calls(), 4);
      sub.unsubscribe();
    }, 60001);
  });
});

test('switching node clears retained snapshots and rejects a late old answer', () => {
  const scheduler = new TestScheduler(assert.deepEqual);
  scheduler.run(() => {
    const old = new Subject();
    let next = of(snapshot);
    const h = setup({ answer: () => next });
    let state;
    const sub = h.panel.state$.subscribe(v => (state = v));
    scheduler.schedule(() => {
      assert.equal(state.data, snapshot);
      next = old;
      h.tip(2);
      next = throwError(() => new Error('new node busy'));
      h.node.next({ url: '/node-b', isWebNode: false });
      old.next(snapshot);
      assert.equal(state.data, null);
      assert.equal(state.unavailable, true);
      sub.unsubscribe();
    }, 1);
  });
});

test('per-block lookup retains its receipt through failed and empty lookups', () => {
  const scheduler = new TestScheduler(assert.deepEqual);
  scheduler.run(() => {
    let mode = 'success';
    const h = setup({
      answer: () =>
        mode === 'success'
          ? of(snapshot)
          : mode === 'failure'
            ? throwError(() => new Error('busy'))
            : of({ ...snapshot, receipts: [] }),
    });
    let state;
    const sub = h.service
      .receipt$(receipt.block.stateHash)
      .subscribe(v => (state = v));
    scheduler.schedule(() => {
      assert.equal(state.receipt, receipt);
      mode = 'failure';
      h.tip(1);
      assert.equal(state.receipt, receipt);
      assert.equal(state.error, true);
      mode = 'empty';
      h.tip(2);
      assert.equal(state.receipt, receipt);
      sub.unsubscribe();
    }, 30);
  });
});

test('valid empty first snapshot is ready, not unavailable; age and capability errors stay explicit', () => {
  const scheduler = new TestScheduler(assert.deepEqual);
  scheduler.run(() => {
    const h = setup({ answer: () => of({ ...snapshot, receipts: [] }) });
    let state;
    const sub = h.panel.state$.subscribe(v => (state = v));
    scheduler.schedule(() => {
      assert.equal(state.data.receipts.length, 0);
      assert.equal(state.unavailable, false);
      assert.equal(state.error, false);
      sub.unsubscribe();
    }, 1);
  });
  assert.equal(poll.updatedSeconds(1000, 4200), 3);
  assert.equal(poll.updatedSeconds(null, 4200), null);
  assert.equal(poll.verificationUnsupported(new Error('timeout')), false);
  assert.equal(
    poll.verificationUnsupported({ name: 'VerificationUnsupportedError' }),
    true,
  );
  assert.equal(
    types.shortReceiptHash('3N12345678901234567890'),
    '3N1234567…67890',
  );
  assert.equal(receipt.blockProof.checkedAt, 1000);
});
