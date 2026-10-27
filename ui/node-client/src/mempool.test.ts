import { expect, test } from "vitest";

import misc from "./fixtures/misc-devnet.json";
import { parsePendingTransaction, parseTransactionPool } from "./mempool";

test("the devnet node's pending payment", () => {
  const [tx] = parseTransactionPool(misc.transaction_pool);
  expect(tx).toEqual({
    hash: "5Jtw2VKyj5ZugpiBn9ojWUY7ZkUN9ALoLAZ76FmFc8gUe9ooffqA",
    kind: "Payment",
    sender: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
    receiver: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
    // Fees come as MINA ("0.01"), amounts as nanomina ("10000000").
    amount: 10_000_000n,
    fee: 10_000_000n,
    nonce: 749856,
    memo: "devnet-tx-generator",
  });
});

test("delegations and zkApp transactions", () => {
  const delegation = parsePendingTransaction({
    hash: "5Jd",
    data: [
      "Signed_command",
      {
        payload: {
          common: {
            fee: "0.0101",
            fee_payer_pk: "B62qa",
            nonce: "3",
            memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH",
          },
          body: [
            "Stake_delegation",
            ["Set_delegate", { new_delegate: "B62qd" }],
          ],
        },
      },
    ],
  });
  expect(delegation).toMatchObject({
    kind: "Delegation",
    receiver: "B62qd",
    amount: null,
    fee: 10_100_000n,
    memo: "",
  });
  const zkapp = parsePendingTransaction({
    hash: "5Jz",
    data: [
      "Zkapp_command",
      {
        fee_payer: { body: { public_key: "B62qz", fee: "0.1", nonce: "7" } },
        memo: "E4Yks7aARFemZJqucP5eaARRYRthGdzaFjGfXqQRS3UeidsECRBvR",
      },
    ],
  });
  expect(zkapp).toMatchObject({
    kind: "zkApp transaction",
    sender: "B62qz",
    fee: 100_000_000n,
    nonce: 7,
    memo: "BorkBorkBork",
  });
  expect(parsePendingTransaction({ data: ["Unknown", {}] })).toBeNull();
  expect(parseTransactionPool(null)).toEqual([]);
});

test("highest fee first", () => {
  const tx = (hash: string, fee: string) => ({
    hash,
    data: [
      "Zkapp_command",
      {
        fee_payer: { body: { public_key: "B62q", fee, nonce: "1" } },
        memo: "",
      },
    ],
  });
  expect(
    parseTransactionPool([tx("a", "0.01"), tx("b", "2"), tx("c", "0.5")]).map(
      t => t.hash,
    ),
  ).toEqual(["b", "c", "a"]);
});
