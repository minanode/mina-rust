import { MINA_TOKEN_ID } from "@minanode/node-client";
import { describe, expect, test, vi } from "vitest";

import {
  big,
  RoleNode,
  parseInclusionLook,
  parseConsensus,
  parseConsensusTime,
  parseSendReceipt,
  parseStake,
  poolHas,
} from "./node";

describe("the role RPCs' answers", () => {
  test("an accepted payment", () => {
    expect(
      parseSendReceipt({
        hash: "5Jt",
        network: "devnet",
        signing: "rust-wasm",
        accepted: true,
        response: [{}],
      }),
    ).toEqual({ hash: "5Jt", accepted: true, rejection: null });
  });

  test("a refused payment: the untagged [command, error] pairs", () => {
    expect(
      parseSendReceipt({
        hash: "5Jt",
        accepted: false,
        response: [[{ signed_command: {} }, "FeePayerAccountNotFound"]],
      }),
    ).toEqual({
      hash: "5Jt",
      accepted: false,
      rejection: "FeePayerAccountNotFound",
    });
    expect(
      parseSendReceipt({
        hash: "5Jt",
        accepted: false,
        response: ["no verifier"],
      }).rejection,
    ).toBe("no verifier");
  });

  test("amounts are exact: decimal strings, and only safe numbers", () => {
    expect(big("18446744073709551615")).toBe(18_446_744_073_709_551_615n);
    expect(big(-1)).toBeNull();
    expect(big("1e3")).toBeNull();
    expect(big(712_410_000_000)).toBe(712_410_000_000n);
  });

  test("an inclusion look: the tip, the search and the find", () => {
    expect(
      parseInclusionLook({
        tip_height: 100,
        depth_searched: 40,
        found: { height: 99, block_hash: "3NA", depth: 1, failure: null },
      }),
    ).toEqual({
      tipHeight: 100,
      depthSearched: 40,
      found: { height: 99, blockHash: "3NA", depth: 1, failure: null },
    });
    expect(
      parseInclusionLook({ tip_height: 100, depth_searched: 40, found: null })
        ?.found,
    ).toBeNull();
    expect(parseInclusionLook(null)).toBeNull();
  });

  test("no hash is an error", () => {
    expect(() => parseSendReceipt({ accepted: true })).toThrow();
  });

  test("stake, constants and time", () => {
    const s = parseStake({
      epoch: 20,
      best_tip_height: 579_000,
      staking: {
        hash: "jx1",
        delegated_stake: 0,
        delegators: 0,
        total_currency: "1000000000000000000",
      },
      next: {
        hash: "jx2",
        delegated_stake: null,
        delegators: null,
        total_currency: "1000000000000000000",
      },
    });
    expect(s?.staking.delegatedStake).toBe(0n);
    expect(s?.next.delegatedStake).toBeNull();
    expect(
      parseConsensus({
        slots_per_epoch: 7140,
        slot_duration_ms: 90_000,
        k: 290,
      }),
    ).toEqual({ slotsPerEpoch: 7140, slotMs: 90_000, k: 290 });
    expect(
      parseConsensusTime({
        epoch: 20,
        global_slot: 143_800,
        slot: 1000,
        start_time: "1800000000000000000",
        end_time: "1800000090000000000",
      }),
    ).toEqual({
      epoch: 20,
      globalSlot: 143_800,
      slot: 1000,
      slotStartMs: 1_800_000_000_000,
    });
  });

  test("the pool holds a hash", () => {
    expect(poolHas([{ hash: "a" }, { hash: "b" }], "b")).toBe(true);
    expect(poolHas([{ hash: "a" }], "b")).toBe(false);
    expect(poolHas(null, "b")).toBe(false);
  });
});

const summary = (balance: unknown = "9007199254740993") => ({
  public_key: "B62q",
  token_id: MINA_TOKEN_ID,
  balance,
  nonce: 6,
  delegate: "B62delegate",
});
function accountNode(raw: unknown) {
  const get = vi.fn(async () => raw);
  const node = new RoleNode({
    ledger: () => ({ latest: () => ({ accounts: () => ({ get }) }) }),
    transaction_pool: () => ({ get: async () => [] }),
  });
  return { node, get };
}

test("roles use the same ledger summary and select MINA, with exact nanomina", async () => {
  const { node, get } = accountNode([
    { ...summary("7"), token_id: "other" },
    summary(),
  ]);
  expect(await node.account("B62q")).toEqual({
    publicKey: "B62q",
    balance: 9_007_199_254_740_993n,
    nonce: 6,
    delegate: "B62delegate",
  });
  expect(get).toHaveBeenCalledExactlyOnceWith("B62q");
});

test("no MINA account is distinct from an unread or invalid account", async () => {
  for (const raw of [[], [{ ...summary(), token_id: "other" }]]) {
    expect(await accountNode(raw).node.account("B62q")).toBeNull();
  }
  for (const raw of [
    null,
    [summary(9_007_199_254_740_992)],
    [{ ...summary(), nonce: null }],
  ]) {
    await expect(accountNode(raw).node.account("B62q")).rejects.toThrow();
  }
});

test("stake sums remain exact after the RPC JSON boundary", () => {
  const ledger = {
    hash: "jx",
    delegated_stake: "9007199254740993",
    delegators: 1,
    total_currency: "18446744073709551615",
  };
  const parsed = parseStake(
    JSON.parse(
      JSON.stringify({
        epoch: 20,
        best_tip_height: 1,
        staking: ledger,
        next: ledger,
      }),
    ),
  );
  expect(parsed?.staking.delegatedStake).toBe(9_007_199_254_740_993n);
  expect(parsed?.next.totalCurrency).toBe(18_446_744_073_709_551_615n);
});
