// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";

import {
  parseSyncProgress,
  type NodeClient,
  type SyncProgress,
} from "@minanode/node-client";

import { Boot, syncSteps, type Step } from "./boot";
import type { BuildConfig } from "./build-config";

const CONFIG: BuildConfig = {
  network: "devnet",
  relays: ["/ip4/192.0.2.1/tcp/3000/webrtc/http/p2p/12D3KooWExample"],
  seedUrls: [],
  genesisUrl: "/genesis.json",
  iceServersUrls: [],
  frontierStore: false,
  glueUrl: "/glue.js",
  wasm: { url: "/node.wasm", sha256: "00", bytes: null },
  startBytes: null,
  commit: null,
};

const step = (
  id: Step["id"],
  state: Step["state"],
  at: number | null,
): Step => ({
  id,
  label: id,
  state,
  detail: null,
  startedAt: at,
  endedAt: state === "done" ? at : null,
});

const active = (steps: Step[]) =>
  steps.filter(s => s.state === "current").map(s => s.id);

afterEach(() => vi.useRealTimers());

test("a restarted sync reopens the ledgers and puts the blocks back", () => {
  let steps = [
    step("ledgers", "pending", null),
    step("blocks", "pending", null),
  ];
  steps = syncSteps(steps, "ledgers", 1_000);
  expect(active(steps)).toEqual(["ledgers"]);
  steps = syncSteps(steps, "blocks", 5_000);
  expect(active(steps)).toEqual(["blocks"]);
  expect(steps[0]).toMatchObject({
    state: "done",
    startedAt: 1_000,
    endedAt: 5_000,
  });

  // A new target: back to the ledgers.
  steps = syncSteps(steps, "ledgers", 7_000);
  expect(active(steps)).toEqual(["ledgers"]);
  // The ledger time runs on from its first start, no longer frozen.
  expect(steps[0]).toMatchObject({ startedAt: 1_000, endedAt: null });
  expect(steps[1]).toMatchObject({
    state: "pending",
    startedAt: null,
    endedAt: null,
  });

  steps = syncSteps(steps, "blocks", 9_000);
  expect(active(steps)).toEqual(["blocks"]);
  expect(steps[0]).toMatchObject({ endedAt: 9_000 });
  expect(steps[1]).toMatchObject({ startedAt: 9_000 });
});

test("the boot follows a retarget through to Synced with one step under way", async () => {
  vi.useFakeTimers();
  const stages: (SyncProgress["stage"] | "Synced")[] = [
    "StakingLedger",
    "Blocks",
    "RootLedger",
    "RootLedger",
    "Blocks",
    "Synced",
  ];
  let poll = 0;
  const stage = () =>
    stages[Math.min(Math.max(poll - 1, 0), stages.length - 1)];
  const client = {
    status: async () => {
      poll += 1;
      const s = stage();
      return {
        phase: s === "Synced" ? "Synced" : "Bootstrap",
        peers: [{ connected: true, status: "Connected" }],
        bestTip: { height: 100 },
        target: { height: 100 },
      };
    },
    ledgerSync: async () => null,
    // The sync record lists its blocks from the start, as the node's does.
    syncBlocks: async () => ({
      kind: "Bootstrap",
      total: 291,
      fetched: 0,
      applied: 0,
    }),
    syncRestore: async () => null,
    syncProgress: async () =>
      parseSyncProgress({
        k: 290,
        stage: stage(),
        blocks:
          stage() === "Blocks"
            ? { total: 3, fetched: 1, applied: 1, failed: 0, cells: "AF." }
            : null,
      }),
  } as unknown as NodeClient;

  const boot = new Boot(CONFIG);
  const seen: string[][] = [];
  boot.subscribe(() => {
    const steps = boot.getSnapshot().steps;
    const current = active(steps).filter(
      id => id === "ledgers" || id === "blocks",
    );
    seen.push(current);
  });
  const done = (
    boot as unknown as { follow(c: NodeClient): Promise<void> }
  ).follow(client);
  for (let i = 0; i < stages.length + 1; i++) {
    await vi.advanceTimersByTimeAsync(1_000);
  }
  await done;

  // Never two sync steps under way at once.
  expect(seen.every(ids => ids.length <= 1)).toBe(true);
  // The retarget was seen: ledgers, blocks, ledgers again, blocks again.
  const order = seen
    .map(ids => ids[0])
    .filter((id, i, all) => id !== undefined && id !== all[i - 1]);
  expect(order).toEqual(["ledgers", "blocks", "ledgers", "blocks"]);
  const snapshot = boot.getSnapshot();
  expect(snapshot.synced).toBe(true);
  const ledgers = snapshot.steps.find(s => s.id === "ledgers")!;
  const blocks = snapshot.steps.find(s => s.id === "blocks")!;
  expect([ledgers.state, blocks.state]).toEqual(["done", "done"]);
  expect(ledgers.endedAt!).toBeLessThanOrEqual(blocks.startedAt!);
});
