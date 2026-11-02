// webrtcPeerStats (crates/p2p/src/service_impl/webrtc/web.js): each open
// connection's peer, candidate types, round trip and bytes, for the web
// node's Peers page, and never an address.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../../../crates/p2p/src/service_impl/webrtc/web.js", import.meta.url), "utf8");

const report = (localType, extra = {}) =>
  new Map([
    ["T", { type: "transport", selectedCandidatePairId: "P", bytesSent: 300, bytesReceived: 9000 }],
    ["P", { type: "candidate-pair", localCandidateId: "L", remoteCandidateId: "R", currentRoundTripTime: 0.0425, bytesSent: 1, bytesReceived: 2 }],
    ["L", { type: "local-candidate", candidateType: localType, address: "203.0.113.7", port: 50000 }],
    ["R", { type: "remote-candidate", candidateType: "host", address: "198.51.100.9", port: 3478 }],
    ...Object.entries(extra),
  ]);

test("labelled, connected peers only, with their selected pair", async () => {
  const Original = globalThis.RTCPeerConnection;
  class Peer {
    signalingState = "stable";
    connectionState = "connected";
    constructor(config) { this.config = config; }
    getConfiguration() { return this.config; }
    async getStats() { return this.stats; }
  }
  globalThis.RTCPeerConnection = Peer;
  try {
    const transport = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    const direct = transport.createWebrtcConnection({ iceServers: [] });
    direct.stats = report("srflx");
    transport.labelWebrtcConnection(direct, "relay-a");
    const turn = transport.createWebrtcConnection({ iceServers: [] });
    // No transport stats (Firefox): the selected candidate pair instead.
    turn.stats = new Map([
      ["P", { type: "candidate-pair", selected: true, localCandidateId: "L", remoteCandidateId: "R", currentRoundTripTime: 0.2, bytesSent: 5, bytesReceived: 6 }],
      ["L", { type: "local-candidate", candidateType: "relay", address: "192.0.2.1" }],
      ["R", { type: "remote-candidate", candidateType: "host", address: "198.51.100.9" }],
    ]);
    transport.labelWebrtcConnection(turn, "relay-b");
    const unlabelled = transport.createWebrtcConnection({ iceServers: [] });
    unlabelled.stats = report("host");
    const connecting = transport.createWebrtcConnection({ iceServers: [] });
    connecting.connectionState = "connecting";
    transport.labelWebrtcConnection(connecting, "relay-c");
    const closed = transport.createWebrtcConnection({ iceServers: [] });
    closed.signalingState = "closed";
    transport.labelWebrtcConnection(closed, "relay-d");
    const failing = transport.createWebrtcConnection({ iceServers: [] });
    failing.getStats = async () => { throw new Error("closed meanwhile"); };
    transport.labelWebrtcConnection(failing, "relay-e");

    const stats = await transport.webrtcPeerStats();
    assert.deepEqual(stats, [
      { peerId: "relay-a", localType: "srflx", remoteType: "host", rttMs: 42.5, bytesSent: 300, bytesReceived: 9000 },
      { peerId: "relay-b", localType: "relay", remoteType: "host", rttMs: 200, bytesSent: 5, bytesReceived: 6 },
    ]);
    assert.doesNotMatch(JSON.stringify(stats), /\d+\.\d+\.\d+\.\d+/);
  } finally {
    globalThis.RTCPeerConnection = Original;
  }
});
