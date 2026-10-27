import { describe, expect, test } from "vitest";

import included3600 from "./fixtures/included-devnet-3600.json";
import misc from "./fixtures/misc-devnet.json";
import {
  confirmation,
  confirmations,
  confirmationTotals,
  CONFIRMATIONS,
  feeComparison,
  feeInput,
  marketProvers,
  parseIncluded,
  type SnarkMarket,
  parseWorkerControl,
  startChange,
  STOP_CHANGE,
  strategyLabel,
  workerView,
  type WorkerControl,
} from "./worker";

const KEY = "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t";

test("the devnet node's worker: allowed, off", () => {
  const c = parseWorkerControl(misc.control);
  expect(c).toMatchObject({
    allowed: true,
    enabled: false,
    publicKey: null,
    fee: null,
    accountCreationFee: 1_000_000_000,
    workerKind: "None",
    job: null,
    recent: [],
    accountExists: null,
    proofsMade: 0,
    zkappJobs: 59,
    bestTipHeight: 578883,
    // Recorded before the node reported it.
    strategy: null,
  });
  expect(workerView(c)).toMatchObject({ label: "Off", tone: "neutral" });
  expect(feeInput(c)).toBe("0.1");
});

// RpcSnarkWorkerControl as crates/node/src/external_snark_worker/
// worker_control.rs serializes it, for a worker proving a merge.
const working = {
  allowed: true,
  enabled: true,
  public_key: KEY,
  fee: 4_000_000,
  account_creation_fee: 1_000_000_000,
  account_exists: false,
  fee_problem:
    "this account doesn't exist yet: fee must be at least 1 MINA, or fund it first",
  worker: { kind: "Working", job_id: "j1", summary: { Merge: 4 } },
  job: {
    job_id: "j1",
    summary: { Merge: 4 },
    zkapp: false,
    submitted: 1792771470000000000,
  },
  recent: [
    {
      job_id: "j0",
      summary: { Tx: 1 },
      zkapp: true,
      time: 1792771460000000000,
      duration_ms: 18_500,
      fee: 4_000_000,
      offered: true,
      error: null,
    },
    {
      job_id: "jx",
      summary: null,
      zkapp: false,
      time: 1792771450000000000,
      duration_ms: null,
      fee: null,
      offered: false,
      error: "prover crashed",
    },
  ],
  paused_for_block: false,
  best_tip_height: 578883,
  zkapp_jobs: 59,
  setting_warning: null,
  proofs_made: 12,
  strategy: "Random",
};

test("a working worker, its job and outcomes", () => {
  const c = parseWorkerControl(working);
  expect(c.job).toEqual({
    jobId: "j1",
    summary: { kind: "Merge", size: 4 },
    zkapp: false,
    submittedMs: 1792771470000,
  });
  expect(c.recent[0]).toMatchObject({
    summary: { kind: "Tx", size: 1 },
    zkapp: true,
    durationMs: 18_500,
    offered: true,
  });
  expect(c.recent[1]).toMatchObject({
    summary: { kind: null },
    fee: null,
    error: "prover crashed",
  });
  expect(c.accountExists).toBe(false);
  expect(c.feeProblem).toMatch(/at least 1 MINA/);
  expect(workerView(c)).toEqual({
    label: "Working",
    tone: "ok",
    detail: "Proving: merge proof.",
  });
  expect(feeInput(c)).toBe("0.004");
});

test("the job strategy, and an unknown one", () => {
  expect(parseWorkerControl(working).strategy).toBe("Random");
  expect(
    parseWorkerControl({ ...working, strategy: "Sequential" }).strategy,
  ).toBe("Sequential");
  expect(
    parseWorkerControl({ ...working, strategy: "RandomOffset" }).strategy,
  ).toBeNull();
  expect(strategyLabel("Random")).toBe("At random (rand, the default)");
  expect(strategyLabel("Sequential")).toBe("In order (seq)");
});

describe("every worker state reads in words", () => {
  const view = (
    worker: unknown,
    extra: Partial<Record<string, unknown>> = {},
  ) =>
    workerView(parseWorkerControl({ ...working, job: null, worker, ...extra }));
  test.each([
    [{ kind: "Starting" }, "Starting", "info"],
    [{ kind: "Idle" }, "Idle", "ok"],
    [{ kind: "WorkReady", job_id: "j" }, "Proof ready", "ok"],
    [{ kind: "WorkError", job_id: "j", error: "x" }, "Job failed", "warn"],
    [{ kind: "Cancelling", job_id: "j" }, "Cancelling", "info"],
    [{ kind: "Killing" }, "Stopping", "info"],
  ])("%o", (worker, label, tone) => {
    expect(view(worker)).toMatchObject({ label, tone });
  });
  test("paused for a block of ours", () => {
    expect(view({ kind: "Idle" }, { paused_for_block: true }).label).toBe(
      "Paused: a block of ours is due",
    );
  });
  test("an error says what failed and whether it restarts", () => {
    const v = view({
      kind: "Error",
      error: { Error: "binary not found" },
      permanent: true,
    });
    expect(v).toMatchObject({ label: "Error", tone: "error" });
    expect(v.detail).toBe(
      "Error: binary not found It won't restart by itself.",
    );
    expect(
      view({ kind: "Error", error: "StartTimeout", permanent: false }).detail,
    ).toBe("StartTimeout");
  });
  test("enabled with no process yet is starting; disabled is off", () => {
    expect(view({ kind: "None" }).label).toBe("Starting");
    expect(view({ kind: "None" }, { enabled: false }).label).toBe("Off");
  });
  test("a node without the field still reads", () => {
    expect(workerView(parseWorkerControl({})).label).toBe("Off");
    expect(parseWorkerControl(null).allowed).toBe(false);
  });
});

describe("the change sent to POST /snarker/control", () => {
  test("start with a key and a fee in MINA, as --snarker-fee takes it", () => {
    expect(startChange(` ${KEY} `, " 0.004 ")).toEqual({
      ok: true,
      change: { enabled: true, public_key: KEY, fee: "0.004" },
    });
    expect(startChange(KEY, "1")).toMatchObject({ ok: true });
  });
  test("refuses what the node would refuse", () => {
    expect(startChange("B62short", "0.1")).toMatchObject({
      ok: false,
      field: "public_key",
    });
    expect(startChange(KEY.replace("B62", "B63"), "0.1")).toMatchObject({
      ok: false,
      field: "public_key",
    });
    for (const fee of ["", "-1", "0.0000000001", "1e3", "abc", "0,5"]) {
      expect(startChange(KEY, fee)).toMatchObject({ ok: false, field: "fee" });
    }
  });
  test("stop", () => {
    expect(STOP_CHANGE).toEqual({ enabled: false });
  });
});

test("the included-work log, empty and with entries", () => {
  const empty = parseIncluded(misc.included);
  expect(empty).toEqual({
    network: "devnet",
    keys: [],
    startedMs: null,
    finalTotals: { blocks: 0, bundles: 0, zkappBundles: 0, fees: 0 },
    pendingTotals: { blocks: 0, bundles: 0, zkappBundles: 0, fees: 0 },
    maxBlocks: 500,
    entries: [],
    bestTipHeight: null,
    market: null,
  });
  const log = parseIncluded({
    ...misc.included,
    keys: [KEY],
    entries: [
      {
        height: 578880,
        state_hash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
        time_ms: 1792771020000,
        producer: KEY,
        prover: KEY,
        bundles: 2,
        zkapp_bundles: 1,
        fees: 8_000_000,
        status: "pending",
      },
      {
        height: 578500,
        state_hash: null,
        time_ms: 1792700000000,
        producer: KEY,
        prover: KEY,
        bundles: 1,
        zkapp_bundles: 0,
        fees: 4_000_000,
        status: "something new",
      },
      { state_hash: "no height" },
    ],
  });
  expect(log.entries).toHaveLength(2);
  expect(log.entries[0]).toMatchObject({
    height: 578880,
    bundles: 2,
    zkappBundles: 1,
    fees: 8_000_000,
    status: "pending",
  });
  expect(log.entries[1].status).toBe("unknown");
});

test("a typed control round-trips its key", () => {
  const c: WorkerControl = parseWorkerControl(working);
  expect(c.publicKey).toBe(KEY);
});

test("confirmations count the blocks on top, and 15 confirm", () => {
  expect(CONFIRMATIONS).toBe(15);
  const at = (height: number, status: "pending" | "final" | "dropped") => ({
    height,
    status,
  });
  expect(confirmations(at(100, "pending"), 110)).toBe(10);
  expect(confirmations(at(100, "pending"), null)).toBeNull();
  expect(confirmations(at(100, "dropped"), 110)).toBeNull();
  expect(confirmation(at(100, "pending"), 114)).toBe("pending");
  expect(confirmation(at(100, "pending"), 115)).toBe("confirmed");
  expect(confirmation(at(100, "pending"), null)).toBe("pending");
  expect(confirmation(at(100, "final"), null)).toBe("confirmed");
  expect(confirmation(at(100, "dropped"), 500)).toBe("dropped");
});

test("an older node's log: no tip or market; totals from another tip", () => {
  // Recorded from devnet-dev (build 86179b938): seven pending blocks.
  const work = parseIncluded(included3600);
  expect(work.bestTipHeight).toBeNull();
  expect(work.market).toBeNull();
  expect(work.entries).toHaveLength(7);
  const newest = work.entries[0].height;
  const oldest = work.entries[6].height;
  // Without a tip nothing is confirmed yet: all pending.
  const none = confirmationTotals(work);
  expect(none.confirmed).toEqual(work.finalTotals);
  expect(none.pending).toMatchObject({ blocks: 7, fees: 92_000_000 });
  // 15 blocks past the oldest, only it is confirmed.
  const some = confirmationTotals(work, oldest + CONFIRMATIONS);
  expect(some.confirmed.blocks).toBe(1);
  expect(some.pending.blocks).toBe(6);
  expect(some.confirmed.fees + some.pending.fees).toBe(92_000_000);
  // Far past the newest, all are.
  const all = confirmationTotals(work, newest + CONFIRMATIONS);
  expect(all.confirmed).toMatchObject({
    blocks: 7,
    bundles: 23,
    fees: 92_000_000,
  });
  expect(all.pending.blocks).toBe(0);
});

test("the market, as a node with it sends it", () => {
  const work = parseIncluded({
    ...misc.included,
    best_tip_height: 579000,
    market: {
      blocks: 291,
      buying_blocks: 7,
      bundles: 130,
      purchases: [
        {
          height: 578990,
          state_hash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
          time_ms: 1792785690000,
          bundles: 120,
          provers: 3,
          min_fee: 4_000_000,
          max_fee: 10_000_000,
        },
        { no: "height" },
      ],
      offers: [
        { prover: KEY, bundles: 20, min_fee: 4_000_000, max_fee: 4_000_000 },
        { bundles: 1 },
      ],
      other_provers: 0,
      highest_other_fee: 0,
    },
  });
  expect(work.bestTipHeight).toBe(579000);
  expect(work.market).toMatchObject({
    blocks: 291,
    buyingBlocks: 7,
    bundles: 130,
    otherProvers: 0,
    highestOtherFee: 0,
  });
  expect(work.market?.purchases).toEqual([
    {
      height: 578990,
      stateHash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
      timeMs: 1792785690000,
      bundles: 120,
      provers: 3,
      minFee: 4_000_000,
      maxFee: 10_000_000,
    },
  ]);
  expect(work.market?.offers).toEqual([
    { prover: KEY, bundles: 20, minFee: 4_000_000, maxFee: 4_000_000 },
  ]);
});

test("the fee comparison says where this worker stands", () => {
  const OTHER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
  const THIRD = "B62qnLVz8wM7MfJsuYbjFf4UWbwrUBEL5ZdawExxxFhnGXB6siqokyM";
  const market = (offers: SnarkMarket["offers"]): SnarkMarket => ({
    blocks: 291,
    buyingBlocks: 0,
    bundles: 0,
    purchases: [],
    offers,
    otherProvers: null,
    highestOtherFee: null,
  });
  const offer = (prover: string, minFee: number, maxFee = minFee) => ({
    prover,
    bundles: 1,
    minFee,
    maxFee,
  });
  const two = market([
    offer(OTHER, 10_000_000),
    offer(THIRD, 5_000_000, 6_000_000),
  ]);
  expect(feeComparison(two, KEY, 4_000_000)).toBe(
    "This worker asks 0.004 MINA a job, less than any other prover: 2 other provers ask 0.005 to 0.01 MINA.",
  );
  expect(feeComparison(two, KEY, 5_000_000)).toContain(
    "level with the cheapest",
  );
  expect(feeComparison(two, KEY, 8_000_000)).toContain(
    "inside the others' range",
  );
  expect(feeComparison(two, KEY, 20_000_000)).toContain(
    "more than any other prover",
  );
  // Our own offer in the pool counts, not the setting.
  const withOurs = market([offer(KEY, 4_000_000), offer(OTHER, 10_000_000)]);
  expect(feeComparison(withOurs, KEY, null)).toBe(
    "This worker asks 0.004 MINA a job, less than any other prover: 1 other prover asks 0.01 MINA.",
  );
  // A worker that's off.
  expect(feeComparison(two, null, null)).toBe(
    "2 provers in the pool ask 0.005 to 0.01 MINA a job.",
  );
  expect(feeComparison(market([offer(KEY, 4_000_000)]), KEY, 4_000_000)).toBe(
    "Only this worker has proofs in the pool, at 0.004 MINA.",
  );
  expect(feeComparison(market([]), null, null)).toBeNull();
});

test("a capped pool: counts and the top fee come from the node", () => {
  const OTHER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
  const OLD = "B62qnLVz8wM7MfJsuYbjFf4UWbwrUBEL5ZdawExxxFhnGXB6siqokyM";
  const capped: SnarkMarket = {
    blocks: 290,
    buyingBlocks: 40,
    bundles: 900,
    purchases: [],
    // The cheapest of 500 others, our key, and a key the worker used before.
    offers: [
      { prover: OTHER, bundles: 2, minFee: 2_000_000, maxFee: 4_000_000 },
      { prover: KEY, bundles: 1, minFee: 4_000_000, maxFee: 4_000_000 },
      { prover: OLD, bundles: 3, minFee: 9_000_000, maxFee: 9_000_000 },
    ],
    otherProvers: 500,
    highestOtherFee: 1_002_000_000,
  };
  expect(feeComparison(capped, KEY, 4_000_000, [OLD, KEY])).toBe(
    "This worker asks 0.004 MINA a job, inside the others' range: 500 other provers ask 0.002 to 1.002 MINA.",
  );
  expect(marketProvers(capped, [OLD, KEY])).toMatchObject({
    otherCount: 500,
    total: 502,
    lowestOtherFee: 2_000_000,
    highestOtherFee: 1_002_000_000,
  });
  // The worker is off: the node's own old proofs aren't "other provers".
  expect(feeComparison(capped, null, null, [OLD, KEY])).toBe(
    "500 provers in the pool ask 0.002 to 1.002 MINA a job.",
  );
});
