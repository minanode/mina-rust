import { expect, test } from "vitest";

import fixture from "./fixtures/status-devnet.json";
import misc from "./fixtures/misc-devnet.json";
import {
  parseBuildInfo,
  parsePeers,
  parseStatus,
  peerCounts,
  peerHost,
} from "./status";

test("reads the devnet node's /status", () => {
  const s = parseStatus(fixture.status);
  expect(s.network).toBe("devnet");
  expect(s.chainId).toBe(
    "ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8",
  );
  expect(s.phase).toBe("Synced");
  expect(s.bestTip).toEqual({
    height: 578883,
    hash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
    globalSlot: 62323,
    timestampMs: 1792771470000,
  });
  expect(s.root?.height).toBe(578593);
  expect(s.target).toBeNull();
  expect(s.snarkPool).toEqual({ jobs: 338, snarks: 261 });
  expect(s.transactionPool.transactions).toBe(1);
  expect(s.blockProducer).toBeNull();
  expect(s.nextWonSlot).toBeNull();
  expect(s.libp2pPort).toBe(8760);
  expect(s.peers).toHaveLength(4);
  expect(peerCounts(s.peers)).toEqual({
    connected: 3,
    connecting: 0,
    disconnected: 1,
    highestTip: 578882,
  });
});

test("an older node without block timestamps or a root still reads", () => {
  const raw = structuredClone(fixture.status) as Record<string, any>;
  delete raw.transition_frontier.best_tip.timestamp_ms;
  delete raw.transition_frontier.best_tip.timestamp;
  delete raw.transition_frontier.root;
  delete raw.next_block_production;
  delete raw.network_info;
  const s = parseStatus(raw);
  expect(s.bestTip?.timestampMs).toBeNull();
  expect(s.bestTip?.height).toBe(578883);
  expect(s.root).toBeNull();
  expect(s.libp2pPort).toBeNull();
});

test("a block's nanosecond timestamp is used when timestamp_ms is absent", () => {
  const raw = structuredClone(fixture.status) as Record<string, any>;
  delete raw.transition_frontier.best_tip.timestamp_ms;
  expect(parseStatus(raw).bestTip?.timestampMs).toBe(1792771470000);
});

test("a phase is read from status when phase is missing, else Pending", () => {
  expect(
    parseStatus({ transition_frontier: { sync: { status: "Bootstrap" } } })
      .phase,
  ).toBe("Bootstrap");
  const empty = parseStatus(null);
  expect(empty.phase).toBe("Pending");
  expect(empty.peers).toEqual([]);
  expect(empty.bestTip).toBeNull();
});

test("a producer's key and next won slot", () => {
  const s = parseStatus({
    ...fixture.status,
    block_producer: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
    next_block_production: {
      generated_from_consensus_at: {
        epoch: 8,
        slot: 5,
        global_slot: 62323,
        global_slot_since_genesis: 921880,
      },
      next_won_slot: {
        epoch: 8,
        slot: 900,
        global_slot: 63218,
        global_slot_since_genesis: 922775,
        start_time_ms: 1792852650000,
        end_time_ms: 1792852740000,
      },
    },
  });
  expect(s.blockProducer).toBe(
    "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
  );
  expect(s.nextWonSlot).toEqual({
    epoch: 8,
    slot: 900,
    globalSlot: 63218,
    startTimeMs: 1792852650000,
  });
});

test("peers: transport, direction, address and tips", () => {
  const [connected] = parsePeers(fixture.status.peers);
  expect(connected).toMatchObject({
    connected: true,
    libp2p: true,
    incoming: false,
    bestTipHeight: 578879,
  });
  expect(peerHost(connected.address)).toBe("34.136.105.213:10003");
  expect(peerHost("/ip6/::1/tcp/8302/p2p/12D3")).toBe("[::1]:8302");
  expect(peerHost(null)).toBeNull();
  expect(parsePeers([{ nope: 1 }, null, "x"])).toEqual([]);
});

test("build info since 0.20.0: the release as the version, describe apart", () => {
  // /build_env of this branch's node after the R020 bump.
  const info = parseBuildInfo({
    time: "2026-10-23T17:30:00Z",
    version: "0.20.0",
    git: {
      commit_time: "2026-10-23T17:20:00Z",
      commit_hash: "d35e9132b0000000000000000000000000000000",
      branch: "task/f-console-dashboard",
      describe: "m11-browser-block-and-snarks-330-gd35e9132b",
    },
  });
  expect(info.version).toBe("0.20.0");
  expect(info.describe).toBe("m11-browser-block-and-snarks-330-gd35e9132b");
  expect(info.commit).toBe("d35e9132b0000000000000000000000000000000");
});

test("build info, as an older node's /build_env gives it", () => {
  expect(parseBuildInfo(misc.build_env)).toEqual({
    version: "m11-browser-block-and-snarks-289-gcdc78990b",
    describe: null,
    commit: "cdc78990b60b4d2e67bea1c40badfa7765ce8339",
    branch: "task/r45-slow-store-load",
    commitTime: "2026-10-23T05:39:59.000000000Z",
    buildTime: "2026-10-23T05:40:05.761476508Z",
  });
  expect(parseBuildInfo({}).commit).toBeNull();
});
