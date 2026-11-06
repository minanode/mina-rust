import { expect, test } from "vitest";

import { parseSyncProgress } from "@minanode/node-client";

import {
  blockStrip,
  ledgerRows,
  ledgerStepSentence,
  rememberLedgers,
  stripSentence,
} from "./progress";

const ledger = (status: string, extra: Record<string, number> = {}) => ({
  status,
  fetched: null,
  total: null,
  accounts_fetched: null,
  accounts_total: null,
  ...extra,
});

test("ledger rows: done, syncing with counts, waiting", () => {
  const syncing = parseSyncProgress({
    k: 290,
    stage: "NextEpochLedger",
    staking_epoch_ledger: ledger("done"),
    next_epoch_ledger: ledger("syncing", {
      fetched: 3000,
      total: 6000,
      accounts_fetched: 2560,
      accounts_total: 5100,
    }),
    root_snarked_ledger: ledger("waiting"),
    root_staged_ledger: ledger("waiting"),
  });
  const memory = rememberLedgers(
    {
      stakingEpoch: parseSyncProgress({
        staking_epoch_ledger: ledger("syncing", { accounts_total: 5000 }),
      }).stakingEpoch,
    },
    syncing,
  );
  const rows = ledgerRows(syncing, memory);
  expect(rows.map(r => [r.label, r.status, r.percent, r.text])).toEqual([
    ["Staking epoch ledger", "done", 100, "5,000 accounts"],
    ["Next epoch ledger", "syncing", 50, "2,560 of 5,100 accounts"],
    ["Root snarked ledger", "waiting", 0, "Waiting"],
    ["Root staged ledger", "waiting", 0, "Waiting"],
  ]);
  expect(ledgerStepSentence(syncing)).toBe("Ledger 2 of 4.");
  // The next epoch ledger's counts are kept for when it is done.
  expect(memory.nextEpoch?.accountsTotal).toBe(5100);
});

test("the staged ledger's parts, and the unknown 0 of 1", () => {
  const at = (fetched: number, total: number) =>
    ledgerRows(
      parseSyncProgress({
        stage: "RootLedger",
        root_staged_ledger: ledger("syncing", { fetched, total }),
      }),
    )[3];
  expect(at(0, 1)).toMatchObject({
    percent: 0,
    text: "Asking a peer for the parts",
  });
  expect(at(3, 12)).toMatchObject({ percent: 25, text: "3 of 12 parts" });
  // Done without counts seen: just done.
  expect(
    ledgerRows(parseSyncProgress({ root_staged_ledger: ledger("done") }))[3]
      .text,
  ).toBe("Done");
});

test("the frontier strip fills from the node's cells", () => {
  const strip = blockStrip(
    parseSyncProgress({
      k: 290,
      stage: "Blocks",
      blocks: { total: 6, fetched: 4, applied: 2, failed: 0, cells: "AAaF.f" },
    }),
  )!;
  expect(strip.cells).toEqual([
    "applied",
    "applied",
    "applying",
    "fetched",
    "missing",
    "fetching",
  ]);
  expect(strip.toGo).toBe(4);
  expect(stripSentence(strip)).toBe("4 of 6 to go: 4 fetched, 2 applied.");
});

test("before the blocks, the frontier to come has k + 1 cells", () => {
  const strip = blockStrip(parseSyncProgress({ k: 290, stage: "RootLedger" }))!;
  expect(strip.waiting).toBe(true);
  expect(strip.cells).toHaveLength(291);
  expect(stripSentence(strip)).toBe(
    "291 blocks to fetch and apply once the ledgers are in.",
  );
  expect(blockStrip(parseSyncProgress({ stage: "RootLedger" }))).toBeNull();
  expect(blockStrip(parseSyncProgress({ k: 290, stage: "Synced" }))).toBeNull();
});
