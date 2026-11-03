import { describe, expect, test } from "vitest";

import type { Account } from "./node";
import { checkPayment, nextStage, rejectionText, type Sent } from "./payment";

const FROM = `B62q${"1".repeat(51)}`;
const TO = `B62q${"2".repeat(51)}`;
const funded: Account = {
  publicKey: FROM,
  balance: 10_000_000_000n,
  nonce: 4,
  delegate: FROM,
};
const ctx = {
  from: FROM,
  sender: funded,
  recipient: { ...funded, publicKey: TO },
  validKey: (k: string) => k === FROM || k === TO,
};
const form = { to: TO, amount: "1.5", fee: "0.1", memo: "hi" };

describe("the payment form", () => {
  test("a good payment, in nanomina", () => {
    expect(checkPayment(form, ctx)).toEqual({
      payment: {
        to: TO,
        amount: 1_500_000_000n,
        fee: 100_000_000n,
        memo: "hi",
      },
      errors: {},
    });
  });

  test("each field's problem", () => {
    const { payment, errors } = checkPayment(
      {
        to: "B62x",
        amount: "1.0000000001",
        fee: "0.0001",
        memo: "m".repeat(33),
      },
      ctx,
    );
    expect(payment).toBeNull();
    expect(Object.keys(errors).sort()).toEqual(["amount", "fee", "memo", "to"]);
  });

  test("a new account needs 1 MINA of the amount", () => {
    const { errors } = checkPayment(
      { ...form, amount: "0.5" },
      {
        ...ctx,
        recipient: null,
      },
    );
    expect(errors.amount).toMatch(/at least 1 MINA/);
  });

  test("the balance covers amount and fee", () => {
    const { errors } = checkPayment({ ...form, amount: "9.95" }, ctx);
    expect(errors.amount).toMatch(/balance/);
  });

  test("a key with no account is told to use the faucet", () => {
    const { errors } = checkPayment(form, { ...ctx, sender: null });
    expect(errors.form).toMatch(/faucet/);
  });

  test("memos count bytes, not characters", () => {
    const { errors } = checkPayment({ ...form, memo: "é".repeat(17) }, ctx);
    expect(errors.memo).toBeDefined();
  });
});

const sent: Sent = {
  hash: "5Jtx",
  from: FROM,
  to: TO,
  amount: 1n,
  fee: 1n,
  nonce: 4,
  memo: "",
  sentAt: 0,
  stage: "pending",
  detail: null,
  inclusion: null,
};

const at = (height: number, depth: number, failure: string | null = null) => ({
  height,
  blockHash: "3N",
  depth,
  failure,
});
/** One look from a tip at 20 over 40 blocks. */
const look = (found: ReturnType<typeof at> | null) => ({
  tipHeight: 20,
  depthSearched: 40,
  found,
});

describe("a sent payment's progress", () => {
  test("pool, block, confirmed", () => {
    expect(
      nextStage(sent, { inclusion: look(null), inPool: true, accountNonce: 4 })
        .stage,
    ).toBe("pending");
    expect(
      nextStage(sent, {
        inclusion: look(at(10, 3)),
        inPool: false,
        accountNonce: 5,
      }).stage,
    ).toBe("included");
    expect(
      nextStage(sent, {
        inclusion: look(at(5, 15)),
        inPool: false,
        accountNonce: 5,
      }).stage,
    ).toBe("confirmed");
  });

  test("included as failed", () => {
    const next = nextStage(sent, {
      inclusion: look(at(10, 0, "Amount_insufficient")),
      inPool: false,
      accountNonce: 5,
    });
    expect(next.stage).toBe("failed");
    expect(next.detail).toContain("Amount_insufficient");
  });

  test("the nonce moved on without it: dropped", () => {
    expect(
      nextStage(sent, { inclusion: look(null), inPool: false, accountNonce: 5 })
        .stage,
    ).toBe("dropped");
  });

  test("a block that leaves the best chain within the search: a reorg", () => {
    const seen = { ...sent, stage: "included" as const, inclusion: at(19, 1) };
    expect(
      nextStage(seen, { inclusion: look(null), inPool: true, accountNonce: 4 }),
    ).toMatchObject({ stage: "pending", inclusion: null });
    expect(
      nextStage(seen, {
        inclusion: look(null),
        inPool: false,
        accountNonce: 5,
      }),
    ).toMatchObject({ stage: "dropped", inclusion: null });
  });

  test("a block older than the search keeps its place", () => {
    // The search from 100 over 40 blocks reaches back to 61.
    const seen = { ...sent, stage: "included" as const, inclusion: at(60, 9) };
    expect(
      nextStage(seen, {
        inclusion: { tipHeight: 100, depthSearched: 40, found: null },
        inPool: false,
        accountNonce: 5,
      }),
    ).toMatchObject({ stage: "included", inclusion: { height: 60 } });
  });

  test("unknown lookups change nothing", () => {
    const seen = { ...sent, stage: "included" as const, inclusion: at(19, 1) };
    for (const inclusion of [undefined, null]) {
      expect(
        nextStage(seen, {
          inclusion,
          inPool: undefined,
          accountNonce: undefined,
        }),
      ).toMatchObject({ stage: "included", inclusion: { height: 19 } });
    }
    expect(
      nextStage(sent, {
        inclusion: look(null),
        inPool: undefined,
        accountNonce: 5,
      }).stage,
    ).toBe("pending");
  });

  test("the pool's reasons in words", () => {
    expect(rejectionText("FeePayerAccountNotFound")).toMatch(/faucet/);
    expect(rejectionText('"InvalidNonce"')).toMatch(/nonce/);
    expect(rejectionText("Something_new")).toBe("Something_new");
  });
});
