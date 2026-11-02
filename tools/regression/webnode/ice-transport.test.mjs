import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../../../crates/p2p/src/service_impl/webrtc/web.js", import.meta.url), "utf8");
const cases = JSON.parse(readFileSync(new URL("../../deploy/minaweb/ice-url-cases.json", import.meta.url)));
test("renewal validates before publication and preserves working and future peers on failure", async () => {
  const Original = globalThis.RTCPeerConnection;
  class Peer {
    signalingState = "stable";
    constructor(config) {
      if (config.iceServers?.some(s => s.urls.includes("turn:browser-rejected.example"))) throw new SyntaxError("browser rejected configuration");
      this.config = structuredClone(config);
    }
    getConfiguration() { return structuredClone(this.config); }
    setConfiguration(config) {
      if (this.reject) throw new Error("live peer rejected configuration");
      this.config = structuredClone(config);
    }
    close() { this.signalingState = "closed"; }
    restartIce() { assert.fail("renewal must not restart ICE"); }
  }
  globalThis.RTCPeerConnection = Peer;
  try {
    const transport = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    for (const url of cases.valid) assert.equal(transport.validIceUrl(url), true);
    for (const url of cases.invalid) assert.equal(transport.validIceUrl(url), false);
    const servers = [{ urls: ["stun:example.org", "turn:example.org"], username: `${Math.floor(Date.now() / 1000) + 600}:fixture`, credential: "fixture" }];
    const first = transport.createWebrtcConnection({ iceServers: servers, iceTransportPolicy: "relay" });
    first.close = () => assert.fail("renewal must not close a working peer");
    const closed = transport.createWebrtcConnection({ iceServers: [] });
    closed.close();
    closed.setConfiguration = () => assert.fail("closed peer updated");
    transport.updateWebrtcIceServers(servers);
    for (const bad of [[], [{ urls: ["stun:example.org"] }],
      ...cases.invalid.map(url => [{ ...servers[0], urls: [url] }]),
      [{ ...servers[0], urls: ["turn:browser-rejected.example"] }],
      [{ ...servers[0], username: "" }], [{ ...servers[0], credential: "" }],
      [{ ...servers[0], username: "1:expired" }], [{ ...servers[0], username: "999999999999:far" }]]) {
      assert.throws(() => transport.updateWebrtcIceServers(bad), /invalid ICE renewal configuration/);
      assert.deepEqual(first.config.iceServers, servers);
      const next = transport.createWebrtcConnection({ iceServers: [] });
      assert.deepEqual(next.config.iceServers, servers);
      next.close();
    }
    const rejector = transport.createWebrtcConnection({ iceServers: servers });
    rejector.reject = true;
    assert.throws(() => transport.updateWebrtcIceServers([{ ...servers[0], credential: "new" }]), /invalid ICE/);
    assert.deepEqual(first.config.iceServers, servers); // Earlier peer rolled back.
    rejector.close();
    transport.updateWebrtcIceServers([{ ...servers[0], credential: "recovered" }]);
    assert.equal(first.config.iceServers[0].credential, "recovered");
    assert.equal(first.config.iceTransportPolicy, "relay");
    servers[0].credential = "mutated caller";
    assert.equal(transport.createWebrtcConnection({ iceServers: [] }).config.iceServers[0].credential, "recovered");
  } finally { globalThis.RTCPeerConnection = Original; }
});
