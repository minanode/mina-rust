import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  hostPort,
  observedTopology,
  shortPeerId,
} from '../src/app/shared/helpers/topology.helper.ts';

// Entries shaped as the soak node's /state/peers answered on 2026-10-21
// (addresses and ids copied from it).
const records = [
  {
    peer_id: '2aPGGj3k52EgTb6z4VuFw2AUL8reAonmjczksVx3hDq1Yzgs1Z4',
    address:
      '/ip4/8.210.56.69/tcp/8302/p2p/12D3KooWA1ftji5qrqodgjLW4sF8mYQhnNSKhm3Zi8bpVTqvd3zk',
    connection_status: 'Connected',
    incoming: false,
  },
  {
    peer_id: '2aSjpibEBdywybnRFH9Tcz79Sy7zfKd4dNEZdAP3zFZfrxCzEMb',
    address:
      '/ip4/162.19.60.166/tcp/8302/p2p/12D3KooWAYUMfNyohEWg533u6A8nz1Y3VDVdNY56cinC4z2cWbC3',
    connection_status: 'Connected',
    incoming: true,
  },
  {
    peer_id: '2bEgBrPTzL8wov2D4Kz34WVLCxR4uCarsBmHYXWKQA5wvBQzd9H',
    address: '/ip4/1.2.3.4/tcp/8302/p2p/12D3KooWFoo',
    connection_status: 'Connecting',
    incoming: false,
  },
  {
    peer_id: '2cDisconnectedPeerIdXXXXXXXXXXXXXXXXXXXXXXXXXXXXX',
    address: null,
    connection_status: 'Disconnected',
  },
];

test('reads host and port from multiaddrs', () => {
  assert.equal(
    hostPort('/ip4/8.210.56.69/tcp/8302/p2p/12D3KooWA1'),
    '8.210.56.69:8302',
  );
  assert.equal(
    hostPort('/ip4/127.0.0.1/tcp/3300/webrtc/http/p2p/12D3KooWRJ'),
    '127.0.0.1:3300',
  );
  assert.equal(hostPort('/ip6/::1/tcp/8302'), '[::1]:8302');
  assert.equal(
    hostPort('/dns4/seed.example.org/tcp/10001'),
    'seed.example.org:10001',
  );
  assert.equal(hostPort('/p2p/12D3KooWA1'), null);
  assert.equal(hostPort(null), null);
});

test('shortens peer ids', () => {
  assert.equal(
    shortPeerId('2aPGGj3k52EgTb6z4VuFw2AUL8reAonmjczksVx3hDq1Yzgs1Z4'),
    '2aPGGj…s1Z4',
  );
  assert.equal(shortPeerId('short'), 'short');
});

test('draws only this node and its connected peers', () => {
  const { peers, links } = observedTopology(records, 'Native mainnet node');
  assert.deepEqual(
    peers.map(p => [p.address, p.node, !!p.self]),
    [
      ['Native mainnet node', 'This node', true],
      ['8.210.56.69:8302', '2aPGGj…s1Z4', false],
      ['162.19.60.166:8302', '2aSjpi…zEMb', false],
    ],
  );
  // Each link from the side that dialed.
  assert.deepEqual(links, [
    { source: 'Native mainnet node', target: '8.210.56.69:8302' },
    { source: '162.19.60.166:8302', target: 'Native mainnet node' },
  ]);
});

test('keeps two peers behind one address apart', () => {
  const same = [
    records[0],
    {
      ...records[0],
      peer_id: '2zOtherPeerIdYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY',
    },
  ];
  const { peers, links } = observedTopology(same, 'n');
  assert.equal(peers.length, 3);
  assert.equal(new Set(peers.map(p => p.address)).size, 3);
  assert.equal(links.length, 2);
});

test('no answer or no connections leaves this node alone', () => {
  assert.deepEqual(observedTopology([], 'n'), {
    peers: [{ address: 'n', node: 'This node', self: true }],
    links: [],
  });
  assert.equal(observedTopology(undefined, '').peers[0].address, 'This node');
});
