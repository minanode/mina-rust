import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import {
  blockCounts,
  commandFees,
  findCommand,
  formatMina,
  slotSinceHardFork,
} from '../src/app/features/chain/chain-format.ts';

// Raw fields captured from the live Mesa node; expected MINA amounts use
// decimal nanomina / 10^9, independently checked with Python Decimal.
const { blocks } = JSON.parse(
  readFileSync(new URL('./fixtures/f1-live-blocks.json', import.meta.url)),
);
const block = height =>
  blocks.find(b => b.protocolState.consensusState.blockHeight === height);

test('formats live payment amounts and fees without losing nanomina', () => {
  assert.equal(formatMina('21103243'), '0.021103243');
  assert.equal(formatMina('10000000'), '0.01');
  assert.equal(formatMina('11100000000'), '11.1');
  assert.equal(formatMina('0'), '0');
});

test('adds all signed command fees in mainnet block 564592', () => {
  assert.equal(commandFees(block('564592')), '0.53');
  assert.deepEqual(blockCounts(block('564592')), {
    payments: 25,
    delegations: 0,
    zkapps: 0,
    works: 115,
  });
});

test('includes zkApp fee payers in total fees', () => {
  assert.equal(blockCounts(block('564394')).zkapps, 3);
  assert.equal(commandFees(block('564394')), '0.24');
});

test('separates payments and stake delegations', () => {
  assert.equal(blockCounts(block('564535')).delegations, 1);
  assert.equal(blockCounts(block('564535')).payments, 6);
});

test('does not present a missing or malformed amount as zero', () => {
  for (const value of [null, undefined, '', 'invalid', '1.2', '-1']) {
    assert.equal(formatMina(value), 'Unavailable');
  }
});

// Two payments of mainnet block 564795 as the soak node's GraphQL returned
// them on 2026-10-21, and a zkApp command hash in the shape GraphQL gives.
const paid = {
  transactions: {
    userCommands: [
      {
        hash: '5JtvDyi6SmVtBS66NegfiFDdokcwaKmuXDwa4yFskWSBuwM7ZEFY',
        isDelegation: false,
        failureReason: null,
      },
      {
        hash: '5JtVb1MkFFsu9ioJPvkniaUb8McWfgduPSZJeF7G3x3yVGS1EY8e',
        isDelegation: true,
        failureReason: 'Amount_insufficient_to_create_account',
      },
    ],
    zkappCommands: [
      { hash: '5JzkappHashXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX' },
    ],
  },
};

test('finds a linked transaction in its block by hash', () => {
  const payment = findCommand(
    paid,
    '5JtvDyi6SmVtBS66NegfiFDdokcwaKmuXDwa4yFskWSBuwM7ZEFY',
  );
  assert.equal(payment.kind, 'Payment');
  assert.equal(payment.command.failureReason, null);
  assert.equal(
    findCommand(paid, '5JtVb1MkFFsu9ioJPvkniaUb8McWfgduPSZJeF7G3x3yVGS1EY8e')
      .kind,
    'Delegation',
  );
  assert.equal(
    findCommand(paid, '5JzkappHashXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX').kind,
    'zkApp command',
  );
});

test('a hash that is not in the block, or none, finds nothing', () => {
  assert.equal(findCommand(paid, '5JtNotInThisBlock'), null);
  assert.equal(findCommand(paid, null), null);
  assert.equal(findCommand(paid, ''), null);
});

// Mainnet block 564795 (3NKammKoyyUMP9VZsVk8Wmts9B4v3yfM1VqyftSWGDgsnn4ZM5WP)
// on 2026-10-21: GraphQL gave epoch 6, slot 3143 and slotSinceGenesis
// 1004423; the same node's /stats/sync gave its global_slot as 45983.
test('the slot since the hard fork matches the node sync statistics', () => {
  assert.equal(slotSinceHardFork({ epoch: '6', slot: '3143' }), 45983);
  // The difference is the Mesa fork point in slots since genesis, 958440.
  assert.equal(
    1004423 - slotSinceHardFork({ epoch: '6', slot: '3143' }),
    958440,
  );
  assert.equal(slotSinceHardFork({ epoch: '0', slot: '0' }), 0);
});

test('a malformed epoch or slot gives no slot', () => {
  assert.equal(slotSinceHardFork({ epoch: '', slot: '1' }), null);
  assert.equal(slotSinceHardFork({ epoch: '6', slot: '-1' }), null);
});
