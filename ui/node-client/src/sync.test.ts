import { describe, expect, test } from "vitest";

import fixture from "./fixtures/status-devnet.json";
import { parseStatus, peerCounts } from "./status";
import { STATUS_STALE_MS, syncView, type SyncInput } from "./sync";

const now = Date.UTC(2026, 9, 23, 12, 0, 0);
const base: SyncInput = {
  phase: "Synced",
  bestTipHeight: 578883,
  bestTipTimestampMs: now - 45_000,
  targetHeight: null,
  highestPeerTip: 578883,
  sampledAt: now - 1_000,
  unreachable: false,
};

describe("the phases F-dash3 reports", () => {
  test("Connecting: no peer yet", () => {
    const v = syncView(
      { ...base, phase: "Connecting", highestPeerTip: null },
      now,
    );
    expect(v).toMatchObject({
      key: "Connecting",
      label: "Connecting",
      tone: "info",
    });
    expect(v.detail).toBe("No peer has connected yet.");
  });

  test("Listening: peers, but no best tip from them", () => {
    const v = syncView(
      { ...base, phase: "Listening", highestPeerTip: null },
      now,
    );
    expect(v).toMatchObject({
      key: "Listening",
      label: "Waiting for peers",
      tone: "info",
    });
  });

  test("Bootstrap names its target when it has one", () => {
    expect(
      syncView({ ...base, phase: "Bootstrap", targetHeight: 578890 }, now)
        .headline,
    ).toBe("Bootstrapping to #578890");
    expect(syncView({ ...base, phase: "Bootstrap" }, now).headline).toBe(
      "Bootstrapping",
    );
  });

  test("Catchup counts the blocks still to apply", () => {
    const v = syncView(
      {
        ...base,
        phase: "Catchup",
        bestTipHeight: 578880,
        targetHeight: 578883,
      },
      now,
    );
    expect(v).toMatchObject({
      key: "Catchup",
      label: "Catching up",
      headline: "Catching up to #578883",
    });
    expect(v.detail).toBe("3 blocks to apply.");
  });

  test("Catchup from the genesis block doesn't count 33,460 blocks to apply", () => {
    // Seen live on devnet right after a bootstrap: the phase is Catchup while
    // the best tip is still the genesis block.
    const v = syncView(
      {
        ...base,
        phase: "Catchup",
        bestTipHeight: 545434,
        targetHeight: 578894,
      },
      now,
    );
    expect(v.headline).toBe("Catching up to #578894");
    expect(v.detail).toBe(
      "Fetching the ledgers and blocks it needs from its peers.",
    );
  });

  test("Synced on a fresh tip level with peers", () => {
    const v = syncView(base, now);
    expect(v).toMatchObject({
      key: "Synced",
      label: "Synced",
      tone: "ok",
      headline: "In sync at #578883",
    });
    expect(v.detail).toBe(
      "Its newest block is 45 s old, and no peer is ahead of it.",
    );
  });

  test("Synced on a tip over an hour old is not shown as synced", () => {
    const v = syncView(
      { ...base, bestTipTimestampMs: now - 3 * 3600_000 },
      now,
    );
    expect(v).toMatchObject({
      key: "NotSynced",
      label: "Not synced",
      tone: "warn",
    });
    expect(v.detail).toBe("Its best tip is 3 h old.");
  });

  test("Synced while peers are far ahead is not shown as synced", () => {
    const v = syncView({ ...base, highestPeerTip: 578900 }, now);
    expect(v.key).toBe("NotSynced");
    expect(v.detail).toBe("Peers are at #578900, 17 blocks ahead.");
  });

  test("Synced with no block time and no peer tip can't be checked", () => {
    const v = syncView(
      { ...base, bestTipTimestampMs: null, highestPeerTip: null },
      now,
    );
    expect(v.key).toBe("NotSynced");
  });

  test("a quiet chain is flagged, not called a fault", () => {
    const v = syncView({ ...base, bestTipTimestampMs: now - 20 * 60_000 }, now);
    expect(v).toMatchObject({ key: "Synced", tone: "warn" });
    expect(v.detail).toMatch(/No new block for 20 min/);
  });

  test("an unknown phase from an older node reads as starting", () => {
    expect(syncView({ ...base, phase: "Pending" }, now)).toMatchObject({
      key: "Starting",
      tone: "neutral",
    });
  });
});

describe("no answer and stale data", () => {
  test("a node that never answered", () => {
    const v = syncView(
      { ...base, phase: null, sampledAt: null, unreachable: true },
      now,
    );
    expect(v).toMatchObject({ key: "NoAnswer", tone: "error" });
  });

  test("a node that stopped answering keeps its last figures, with their age", () => {
    const v = syncView(
      { ...base, sampledAt: now - 65_000, unreachable: true },
      now,
    );
    expect(v).toMatchObject({ key: "NoAnswer", stale: true });
    expect(v.detail).toBe(
      "Its last answer came 1 min 5 s ago, at #578883. The figures below are from then.",
    );
  });

  test("an old sample is stale even before a request fails", () => {
    const v = syncView({ ...base, sampledAt: now - STATUS_STALE_MS - 1 }, now);
    expect(v.stale).toBe(true);
    expect(syncView(base, now).stale).toBe(false);
  });
});

test("the devnet node's /status reads as synced, a block above its peers", () => {
  const s = parseStatus(fixture.status);
  const peers = peerCounts(s.peers);
  const tipTime = s.bestTip!.timestampMs!;
  const v = syncView(
    {
      phase: s.phase,
      bestTipHeight: s.bestTip!.height,
      bestTipTimestampMs: tipTime,
      targetHeight: s.target?.height ?? null,
      highestPeerTip: peers.highestTip,
      sampledAt: tipTime + 10_000,
      unreachable: false,
    },
    tipTime + 10_000,
  );
  expect(v).toMatchObject({
    key: "Synced",
    headline: "In sync at #578883",
    peerDelta: -1,
  });
  expect(v.detail).toBe(
    "Its newest block is 10 s old; the highest peer tip is 1 block below ours.",
  );
});
