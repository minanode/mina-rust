import { expect, test } from "vitest";

import { parseVerification } from "@minanode/node-client";

import { recentReceipts, shownReceipts } from "./blocks";

const receipt = (hash: string, height: number, proof = "notRun") => ({
  block: { stateHash: hash, height },
  arrival: "bootstrap",
  receivedAt: 1,
  applied: true,
  blockProof: { status: proof },
});

test("the checked best tip shows above the bootstrap blocks", () => {
  const v = parseVerification({
    lastVerified: { ...receipt("tip", 12, "passed"), arrival: "bestTip" },
    receipts: [receipt("a", 10), receipt("b", 11)],
  });
  expect(shownReceipts(v).map(r => r.stateHash)).toEqual(["tip", "b", "a"]);
  const once = parseVerification({
    lastVerified: receipt("b", 11, "passed"),
    receipts: [receipt("a", 10), receipt("b", 11, "passed")],
  });
  expect(shownReceipts(once).map(r => r.stateHash)).toEqual(["b", "a"]);
});

test("pages of receipts until a short one, at most three", async () => {
  const asked: unknown[] = [];
  const page = (n: number, from: number) =>
    parseVerification({
      receipts: Array.from({ length: n }, (_, i) =>
        receipt(`h${from + i}`, from + i),
      ),
    });
  const client = {
    verification: async (q?: number | { limit?: number; offset?: number }) => {
      asked.push(q);
      const offset = typeof q === "object" ? (q.offset ?? 0) : 0;
      return page(offset < 256 ? 128 : 10, offset);
    },
  };
  const v = await recentReceipts(client);
  expect(v.receipts).toHaveLength(266);
  expect(asked).toEqual([
    { limit: 128 },
    { limit: 128, offset: 128 },
    { limit: 128, offset: 256 },
  ]);
  expect(shownReceipts(v, 3).map(r => r.height)).toEqual([265, 264, 263]);
});
