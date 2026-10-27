/**
 * The Network > Topology graph from what the node itself reports (R28, after
 * R27 found it drew 100 generated peers). A node knows only its own
 * connections, so the graph is a star: this node in the middle and the peers
 * it is connected to now, each link drawn from the side that dialed. Links
 * between other peers are not known and are not drawn. Pure functions, so
 * that they can be tested without Angular (frontend/tests/topology.test.mjs).
 */

/** One entry of the node's `/state/peers` answer, the fields used here. */
export interface PeerRecord {
  peer_id: string;
  address?: string | null;
  connection_status: string;
  incoming?: boolean | null;
  is_libp2p?: boolean | null;
}

export interface TopologyPeer {
  address: string;
  node: string;
  self?: boolean;
  peerId?: string;
}

export interface Topology {
  peers: TopologyPeer[];
  links: { source: string; target: string }[];
}

/** `2aPGGj…1Z4`: enough of a peer id to tell peers apart on screen. */
export function shortPeerId(peerId: string): string {
  return peerId.length > 12
    ? `${peerId.slice(0, 6)}…${peerId.slice(-4)}`
    : peerId;
}

/**
 * `host:port` from a multiaddr (`/ip4/8.210.56.69/tcp/8302/p2p/…` →
 * `8.210.56.69:8302`), or null when it has no host.
 */
export function hostPort(multiaddr: string | null | undefined): string | null {
  if (!multiaddr) {
    return null;
  }
  const parts = multiaddr.split('/').filter(Boolean);
  let host: string | null = null;
  let port: string | null = null;
  for (let i = 0; i + 1 < parts.length; i += 2) {
    const [protocol, value] = [parts[i], parts[i + 1]];
    if (['ip4', 'dns', 'dns4', 'dns6'].includes(protocol) && host === null) {
      host = value;
    } else if (protocol === 'ip6' && host === null) {
      host = `[${value}]`;
    } else if (['tcp', 'udp'].includes(protocol) && port === null) {
      port = value;
    }
  }
  if (host === null) {
    return null;
  }
  return port === null ? host : `${host}:${port}`;
}

/**
 * This node and the peers it is connected to now. Peers that are only
 * connecting or were disconnected are left out.
 */
export function observedTopology(
  records: PeerRecord[],
  selfName: string,
): Topology {
  const self: TopologyPeer = {
    address: selfName || 'This node',
    node: 'This node',
    self: true,
  };
  const used = new Set<string>([self.address]);
  const peers: TopologyPeer[] = [self];
  const links: Topology['links'] = [];
  for (const record of records ?? []) {
    if (record.connection_status !== 'Connected') {
      continue;
    }
    const name = shortPeerId(record.peer_id);
    let address = hostPort(record.address) ?? name;
    if (used.has(address)) {
      // Two peers behind one address: keep both, told apart by their ids.
      address = `${address} (${name})`;
    }
    used.add(address);
    peers.push({ address, node: name, peerId: record.peer_id });
    links.push(
      record.incoming
        ? { source: address, target: self.address }
        : { source: self.address, target: address },
    );
  }
  return { peers, links };
}
