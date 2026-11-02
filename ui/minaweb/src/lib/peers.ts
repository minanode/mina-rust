/** The Peers page's rows: one per relay the build dials, connected or not,
 * and any other peer apart (MINAWEB-PEERS-PAGE). */

import { num, obj, str, type Peer } from "@minanode/node-client";

/** A connection's route and traffic, from the browser's `getStats` on the
 * page's own RTCPeerConnection (crates/p2p/src/service_impl/webrtc/web.js).
 * It carries candidate types only, never an address. */
export interface RouteStats {
  /** Direct (host, srflx or prflx candidates) or through TURN (relay). */
  route: "direct" | "turn" | null;
  rttMs: number | null;
  bytesSent: number | null;
  bytesReceived: number | null;
}

const CANDIDATE_TYPES = new Set(["host", "srflx", "prflx", "relay"]);

/** `webrtc_peer_stats()`'s array, keyed by the node's peer ID. */
export function parseRouteStats(raw: unknown): Map<string, RouteStats> {
  const stats = new Map<string, RouteStats>();
  if (!Array.isArray(raw)) return stats;
  for (const item of raw) {
    const s = obj(item);
    const peerId = str(s?.peerId);
    if (!s || !peerId) continue;
    const local = str(s.localType);
    const remote = str(s.remoteType);
    const known =
      local !== null &&
      remote !== null &&
      CANDIDATE_TYPES.has(local) &&
      CANDIDATE_TYPES.has(remote);
    stats.set(peerId, {
      route: !known
        ? null
        : local === "relay" || remote === "relay"
          ? "turn"
          : "direct",
      rttMs: num(s.rttMs),
      bytesSent: num(s.bytesSent),
      bytesReceived: num(s.bytesReceived),
    });
  }
  return stats;
}

const ADDRESS = /^\/(ip4|ip6|dns4|dns6|dns)\/([^/]+)\/tcp\/(\d+)\//;

/** A relay's name: its domain (with the port unless 443), or address. */
export function relayName(multiaddr: string): string {
  const match = ADDRESS.exec(multiaddr);
  if (!match) return multiaddr;
  const [, family, host, port] = match;
  if (family.startsWith("dns"))
    return port === "443" ? host : `${host}:${port}`;
  return family === "ip6" ? `[${host}]:${port}` : `${host}:${port}`;
}

/** The libp2p peer ID a multiaddr ends with. */
export function multiaddrPeerId(multiaddr: string | null): string | null {
  return multiaddr ? (/\/p2p\/(\w+)$/.exec(multiaddr)?.[1] ?? null) : null;
}

export type RelayState = "connected" | "connecting" | "retrying";

export interface RelayRow {
  name: string;
  state: RelayState;
  peer: Peer | null;
  stats: RouteStats | null;
  /** Connected: since when. Not connected: when this page last saw it
   * connected, if it did. */
  sinceMs: number | null;
}

/**
 * Every relay the build dials, in its order. The node keeps redialling a
 * relay it lost: its minimum peer count is far above the relays it has.
 */
export function relayRows(
  relays: string[],
  peers: Peer[],
  stats: Map<string, RouteStats>,
  lastSeen: Map<string, number>,
): RelayRow[] {
  return relays.map(relay => {
    const id = multiaddrPeerId(relay);
    const peer =
      peers.find(p => id !== null && multiaddrPeerId(p.address) === id) ?? null;
    const state: RelayState = peer?.connected
      ? "connected"
      : peer?.status === "Connecting"
        ? "connecting"
        : "retrying";
    return {
      name: relayName(relay),
      state,
      peer,
      stats: peer?.connected ? (stats.get(peer.peerId) ?? null) : null,
      sinceMs:
        state === "connected"
          ? (peer?.sinceMs ?? null)
          : (lastSeen.get(relay) ?? null),
    };
  });
}

/** Connected peers that are none of the build's relays: other browsers,
 * which only sharing (boot/sharing.ts) or experimental peering connects. */
export function otherPeers(relays: string[], peers: Peer[]): Peer[] {
  const ids = new Set(relays.map(multiaddrPeerId));
  return peers.filter(p => p.connected && !ids.has(multiaddrPeerId(p.address)));
}

/** Record when each relay was last seen connected, for the rows that lose
 * it. Kept for the tab's life, so it survives leaving the page. */
export function noteLastSeen(
  lastSeen: Map<string, number>,
  relays: string[],
  peers: Peer[],
  now: number,
): void {
  for (const relay of relays) {
    const id = multiaddrPeerId(relay);
    if (
      peers.some(
        p => p.connected && id !== null && multiaddrPeerId(p.address) === id,
      )
    ) {
      lastSeen.set(relay, now);
    }
  }
}
