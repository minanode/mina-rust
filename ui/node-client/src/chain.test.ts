import { expect, test } from "vitest";

import fixture from "./fixtures/f1-live-blocks.json";
import {
  ACCOUNT,
  BLOCK_BY_HASH,
  BLOCK_BY_HEIGHT,
  blockCounts,
  blockQuery,
  commandFees,
  isTimed,
  parseAccount,
  parseBestChain,
  parseBlock,
  RECENT_BLOCKS,
} from "./chain";
import { formatMina } from "./format";

// Captured from the live Mesa node (frontend/tests/fixtures); the Angular
// chain tests checked these MINA sums with Python Decimal.
const block = (height: string) =>
  parseBlock(
    fixture.blocks.find(
      b => b.protocolState.consensusState.blockHeight === height,
    ),
  )!;

test("fees and counts of mainnet block 564592", () => {
  const b = block("564592");
  expect(formatMina(commandFees(b))).toBe("0.53");
  expect(blockCounts(b)).toEqual({
    payments: 25,
    delegations: 0,
    zkapps: 0,
    works: 115,
  });
});

test("zkApp fee payers count in a block's fees", () => {
  const b = block("564394");
  expect(blockCounts(b).zkapps).toBe(3);
  expect(formatMina(commandFees(b))).toBe("0.24");
});

test("a full block answer", () => {
  const b = parseBlock({
    stateHash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
    creator: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
    protocolState: {
      previousStateHash: "3NKQTwfLHFim3uLeiF1s6H4BxKVj64g4J96zVDGUcGLBZsaJ1kmE",
      consensusState: {
        blockHeight: "578883",
        slotSinceGenesis: "921880",
        slot: "5",
        epoch: "8",
      },
      blockchainState: {
        snarkedLedgerHash: "jx1",
        stagedLedgerHash: "jx2",
        date: "1792771470000",
        utcDate: "1792771470000",
      },
    },
    transactions: {
      coinbase: "720000000000",
      coinbaseReceiverAccount: {
        publicKey: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
      },
      feeTransfer: [
        { recipient: "B62q", fee: "10000000", type: "Fee_transfer" },
      ],
      userCommands: [
        {
          hash: "5J1",
          from: "B62a",
          to: "B62b",
          amount: "10000000",
          fee: "10000000",
          memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH",
          nonce: 4,
          isDelegation: false,
          failureReason: null,
        },
        {
          hash: "5J2",
          from: "B62a",
          to: "B62c",
          amount: null,
          fee: "20000000",
          memo: "",
          nonce: 5,
          isDelegation: true,
          failureReason: "Amount_insufficient_to_create_account",
        },
      ],
      zkappCommands: [],
    },
    snarkJobs: [{ prover: "B62p", fee: "1000000" }],
  })!;
  expect(b).toMatchObject({
    height: 578883,
    epoch: 8,
    slot: 8 * 7140 + 5,
    slotSinceGenesis: 921880,
    timeMs: 1792771470000,
    coinbase: "720000000000",
  });
  expect(b.userCommands[1]).toMatchObject({
    kind: "Delegation",
    amount: null,
    failure: "Amount_insufficient_to_create_account",
  });
  expect(formatMina(commandFees(b))).toBe("0.03");
  expect(parseBlock({ creator: "x" })).toBeNull();
});

test("bestChain comes oldest first; the list is newest first", () => {
  const list = parseBestChain({ bestChain: fixture.blocks.slice(0, 3) });
  const heights = list.map(b => b.height);
  expect(heights).toEqual([...heights].sort((a, b) => (b ?? 0) - (a ?? 0)));
  expect(parseBestChain({ bestChain: null })).toEqual([]);
});

test("the detail route takes a height or a state hash", () => {
  expect(blockQuery("578883")).toEqual({
    query: BLOCK_BY_HEIGHT,
    variables: { height: 578883 },
  });
  expect(
    blockQuery("3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W"),
  ).toEqual({
    query: BLOCK_BY_HASH,
    variables: {
      stateHash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
    },
  });
  for (const bad of ["", "abc", "3Nshort", "-1", "12345678901"])
    expect(blockQuery(bad)).toBeNull();
});

test("the queries are the Angular explorer's", () => {
  expect(RECENT_BLOCKS).toMatch(
    /^query RecentBlocks\(\$maxLength: Int!\) \{ bestChain\(maxLength: \$maxLength\)/,
  );
  expect(BLOCK_BY_HASH).toMatch(/block\(stateHash: \$stateHash\)/);
  expect(ACCOUNT).toMatch(/account\(publicKey: \$publicKey\)/);
});

test("accounts: found, timed, and missing", () => {
  const a = parseAccount({
    account: {
      publicKey: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
      nonce: "749856",
      balance: {
        total: "1234500000000",
        liquid: "1000000000000",
        locked: "234500000000",
        blockHeight: "578883",
        stateHash: "3N",
      },
      delegateAccount: {
        publicKey: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
      },
      timing: {
        initialMinimumBalance: "500000000000",
        cliffTime: "100",
        cliffAmount: "1",
        vestingPeriod: "1",
        vestingIncrement: "2",
      },
    },
  })!;
  expect(a).toMatchObject({
    nonce: 749856,
    balance: "1234500000000",
    balanceHeight: 578883,
  });
  expect(formatMina(a.balance)).toBe("1,234.5");
  expect(isTimed(a)).toBe(true);
  expect(parseAccount({ account: null })).toBeNull();
  const untimed = parseAccount({
    account: {
      publicKey: "B62q",
      balance: { total: "1" },
      timing: { initialMinimumBalance: null },
    },
  })!;
  expect(isTimed(untimed)).toBe(false);
  expect(untimed.delegate).toBeNull();
});
