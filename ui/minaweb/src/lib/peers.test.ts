import { expect, test } from "vitest";

import type { Peer } from "@minanode/node-client";

import {
  multiaddrPeerId,
  noteLastSeen,
  otherPeers,
  parseRouteStats,
  relayName,
  relayRows,
} from "./peers";

const R1 = "12D3KooWC4k4X9Ub9JBbLWov3UCH1q4MH7iVEZM2dVfx1dhnYSMo";
const R2 = "12D3KooWPoyPpRmXL7fmdNEpRxYkTgnBuJqvjgqQ1Xcx3HtEwtVd";
const RELAY1 = `/dns4/relay1.minaweb.org/tcp/443/webrtc/https/p2p/${R1}`;
const RELAY2 = `/ip4/127.0.0.1/tcp/3540/webrtc/http/p2p/${R2}`;

const peer = (over: Partial<Peer>): Peer => ({
  peerId: "2bEgBrPTzL8wov2D4Kz34WVLCxR4uCarsBmHYXWKQA5wvBQzd9H",
  address: null,
  status: "Connected",
  connected: true,
  incoming: false,
  libp2p: false,
  bestTipHeight: 600_000,
  bestTipHash: null,
  bestTipTimestampMs: null,
  sinceMs: 1_000,
  ...over,
});

test("a relay's name: its domain, or its address and port", () => {
  expect(relayName(RELAY1)).toBe("relay1.minaweb.org");
  expect(relayName(RELAY2)).toBe("127.0.0.1:3540");
  expect(
    relayName(`/dns4/relay2.minaweb.org/tcp/8443/webrtc/https/p2p/${R2}`),
  ).toBe("relay2.minaweb.org:8443");
  expect(relayName(`/ip6/::1/tcp/3540/webrtc/http/p2p/${R2}`)).toBe(
    "[::1]:3540",
  );
  expect(multiaddrPeerId(RELAY1)).toBe(R1);
  expect(multiaddrPeerId(null)).toBeNull();
});

test("the route is TURN when either selected candidate is a relay one", () => {
  const stats = parseRouteStats([
    {
      peerId: "a",
      localType: "srflx",
      remoteType: "host",
      rttMs: 41.5,
      bytesSent: 10,
      bytesReceived: 20,
    },
    {
      peerId: "b",
      localType: "relay",
      remoteType: "host",
      rttMs: null,
      bytesSent: 1,
      bytesReceived: 2,
    },
    { peerId: "c", localType: null, remoteType: null },
    { localType: "host" },
    "junk",
  ]);
  expect([...stats.keys()]).toEqual(["a", "b", "c"]);
  expect(stats.get("a")).toEqual({
    route: "direct",
    rttMs: 41.5,
    bytesSent: 10,
    bytesReceived: 20,
  });
  expect(stats.get("b")?.route).toBe("turn");
  expect(stats.get("c")).toEqual({
    route: null,
    rttMs: null,
    bytesSent: null,
    bytesReceived: null,
  });
  expect(parseRouteStats(undefined).size).toBe(0);
});

test("one row per relay, matched by peer ID, whether connected or not", () => {
  const up = peer({ peerId: "mina-r2", address: RELAY2, sinceMs: 5_000 });
  const stats = parseRouteStats([
    { peerId: "mina-r2", localType: "host", remoteType: "host", rttMs: 1 },
  ]);
  const seen = new Map([[RELAY1, 3_000]]);
  const rows = relayRows([RELAY1, RELAY2], [up], stats, seen);
  expect(rows.map(r => [r.name, r.state, r.sinceMs])).toEqual([
    ["relay1.minaweb.org", "retrying", 3_000],
    ["127.0.0.1:3540", "connected", 5_000],
  ]);
  expect(rows[0].stats).toBeNull();
  expect(rows[1].stats?.route).toBe("direct");

  const dialling = peer({
    address: RELAY1,
    status: "Connecting",
    connected: false,
  });
  expect(relayRows([RELAY1], [dialling], stats, new Map())[0]).toMatchObject({
    state: "connecting",
    sinceMs: null,
  });
  const lost = peer({
    address: RELAY1,
    status: "Disconnected",
    connected: false,
  });
  expect(relayRows([RELAY1], [lost], stats, new Map())[0].state).toBe(
    "retrying",
  );
});

test("other browsers: connected peers that are no relay", () => {
  const relay = peer({ peerId: "r", address: RELAY1 });
  const browser = peer({ peerId: "b", address: null });
  const gone = peer({ peerId: "g", connected: false, status: "Disconnected" });
  expect(otherPeers([RELAY1], [relay, browser, gone])).toEqual([browser]);
});

test("a relay's last connection is kept for when it drops", () => {
  const seen = new Map<string, number>();
  noteLastSeen(seen, [RELAY1, RELAY2], [peer({ address: RELAY1 })], 7_000);
  expect([...seen]).toEqual([[RELAY1, 7_000]]);
  noteLastSeen(
    seen,
    [RELAY1],
    [peer({ address: RELAY1, connected: false, status: "Disconnected" })],
    9_000,
  );
  expect(seen.get(RELAY1)).toBe(7_000);
});
