/** `GET /status` and `GET /build_env`, read into what the pages show. */

import { arr, bool, nanosToMs, num, obj, str, type Json } from "./json";

export interface TipSummary {
  height: number;
  hash: string | null;
  globalSlot: number | null;
  /** The block's own timestamp, ms since the epoch; null from nodes before
   * it was reported. */
  timestampMs: number | null;
}

export interface Peer {
  peerId: string;
  address: string | null;
  status: string;
  connected: boolean;
  incoming: boolean | null;
  libp2p: boolean | null;
  bestTipHeight: number | null;
  bestTipHash: string | null;
  bestTipTimestampMs: number | null;
  /** When the connection state last changed, ms since the epoch. */
  sinceMs: number | null;
}

export interface ConsensusSlot {
  epoch: number;
  slot: number;
  globalSlot: number | null;
  /** When the slot starts, if the node says. */
  startTimeMs: number | null;
}

export interface NodeStatus {
  network: string | null;
  chainId: string | null;
  uptimeSecs: number | null;
  /** The phase the node reports (F-dash3): Connecting, Listening,
   * Bootstrap, Catchup or Synced; older nodes may say something else. */
  phase: string;
  /** When the node's sync state last changed, ms since the epoch. */
  syncTimeMs: number | null;
  bestTip: TipSummary | null;
  root: TipSummary | null;
  /** The block a syncing node syncs to. */
  target: TipSummary | null;
  peers: Peer[];
  snarkPool: { jobs: number | null; snarks: number | null };
  transactionPool: { transactions: number | null };
  /** The producer's public key; null when the node produces no blocks. */
  blockProducer: string | null;
  coinbaseReceiver: string | null;
  /** The next won slot, once the VRF evaluator has found one. */
  nextWonSlot: ConsensusSlot | null;
  libp2pPort: number | null;
}

function tip(value: unknown): TipSummary | null {
  const t = obj(value);
  const height = num(t?.height);
  if (!t || height === null) return null;
  return {
    height,
    hash: str(t.hash),
    globalSlot: num(t.global_slot),
    timestampMs: num(t.timestamp_ms) ?? nanosToMs(t.timestamp),
  };
}

export function parsePeer(value: unknown): Peer | null {
  const p = obj(value);
  const peerId = str(p?.peer_id);
  if (!p || !peerId) return null;
  const status = str(p.connection_status) ?? "Unknown";
  return {
    peerId,
    address: str(p.address),
    status,
    connected: status === "Connected",
    incoming: bool(p.incoming),
    libp2p: bool(p.is_libp2p),
    bestTipHeight: num(p.best_tip_height),
    bestTipHash: str(p.best_tip),
    bestTipTimestampMs: nanosToMs(p.best_tip_timestamp),
    sinceMs: nanosToMs(p.time),
  };
}

export function parsePeers(raw: unknown): Peer[] {
  return arr(raw)
    .map(parsePeer)
    .filter((p): p is Peer => p !== null);
}

function slot(value: unknown): ConsensusSlot | null {
  const s = obj(value);
  const epoch = num(s?.epoch);
  const inEpoch = num(s?.slot);
  if (!s || epoch === null || inEpoch === null) return null;
  return {
    epoch,
    slot: inEpoch,
    globalSlot: num(s.global_slot),
    startTimeMs: num(s.start_time_ms),
  };
}

export function parseStatus(raw: unknown): NodeStatus {
  const s: Json = obj(raw) ?? {};
  const frontier = obj(s.transition_frontier) ?? {};
  const sync = obj(frontier.sync) ?? {};
  const snarkPool = obj(s.snark_pool) ?? {};
  const txPool = obj(s.transaction_pool) ?? {};
  return {
    network: str(s.network),
    chainId: str(s.chain_id),
    uptimeSecs: num(s.uptime_secs),
    phase: str(sync.phase) ?? str(sync.status) ?? "Pending",
    syncTimeMs: nanosToMs(sync.time),
    bestTip: tip(frontier.best_tip),
    root: tip(frontier.root),
    target: tip(sync.target),
    peers: parsePeers(s.peers),
    snarkPool: {
      jobs: num(snarkPool.total_jobs),
      snarks: num(snarkPool.snarks),
    },
    transactionPool: { transactions: num(txPool.transactions) },
    blockProducer: str(s.block_producer),
    coinbaseReceiver: str(s.coinbase_receiver),
    nextWonSlot: slot(obj(s.next_block_production)?.next_won_slot),
    libp2pPort: num(obj(s.network_info)?.libp2p_port),
  };
}

export interface PeerCounts {
  connected: number;
  connecting: number;
  disconnected: number;
  /** The highest best tip a connected peer reports. */
  highestTip: number | null;
}

export function peerCounts(peers: Peer[]): PeerCounts {
  let connected = 0;
  let connecting = 0;
  let highestTip: number | null = null;
  for (const peer of peers) {
    if (peer.connected) {
      connected += 1;
      if (
        peer.bestTipHeight !== null &&
        (highestTip === null || peer.bestTipHeight > highestTip)
      ) {
        highestTip = peer.bestTipHeight;
      }
    } else if (peer.status === "Connecting") {
      connecting += 1;
    }
  }
  return {
    connected,
    connecting,
    disconnected: peers.length - connected - connecting,
    highestTip,
  };
}

export interface BuildInfo {
  /** The release, `0.20.0`; older nodes sent `git describe` here. */
  version: string | null;
  /** `git describe` of the build, from nodes since 0.20.0. */
  describe: string | null;
  commit: string | null;
  branch: string | null;
  commitTime: string | null;
  buildTime: string | null;
}

export function parseBuildInfo(raw: unknown): BuildInfo {
  const b = obj(raw) ?? {};
  const git = obj(b.git) ?? {};
  return {
    version: str(b.version),
    describe: str(git.describe),
    commit: str(git.commit_hash),
    branch: str(git.branch),
    commitTime: str(git.commit_time),
    buildTime: str(b.time),
  };
}

/** Where a peer is, from its multiaddr: `65.21.68.205:8302`. */
export function peerHost(address: string | null): string | null {
  if (!address) return null;
  const match = /^\/(ip4|ip6|dns4|dns6|dns)\/([^/]+)\/(?:tcp|udp)\/(\d+)/.exec(
    address,
  );
  if (!match) return address;
  const [, family, host, port] = match;
  return family === "ip6" ? `[${host}]:${port}` : `${host}:${port}`;
}
