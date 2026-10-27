import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  receiptLabel,
  checkLabel,
} from '../src/app/features/verification/verification.types.ts';

// Telemetry contract tests, not cryptographic vectors. Real proof/check results
// are validated by the native tests and the live browser harness.
const receipt = (status, arrival = 'bootstrap', applied = true) => ({
  blockProof: { status },
  arrival,
  applied,
});
test('bootstrap and catch-up application cannot imply proof verification', () => {
  assert.equal(
    receiptLabel(receipt('notRun')),
    'Applied from bootstrap, not individually verified',
  );
  assert.equal(
    receiptLabel(receipt('notRun', 'catchup')),
    'Applied, not individually verified',
  );
  assert.equal(receiptLabel(), 'No retained receipt');
});
test('only a passed proof receives the positive proof label', () => {
  assert.equal(
    receiptLabel(receipt('passed', 'gossip')),
    'Proof verified here',
  );
  assert.equal(receiptLabel(receipt('failed')), 'Block proof failed');
  assert.equal(receiptLabel(receipt('skipped')), 'Proof check skipped');
  assert.equal(receiptLabel(receipt('pending')), 'Proof verification pending');
});
test('all check states have explicit labels', () => {
  assert.deepEqual(
    ['notRun', 'pending', 'passed', 'failed', 'skipped'].map(status =>
      checkLabel({ status }),
    ),
    ['Not run', 'Pending', 'Passed', 'Failed', 'Skipped'],
  );
});
