import { describe, expect, test } from "vitest";

import { readBuildConfig } from "./build-config";

const SHA = "d6e56ad1eab629c107f704fee96e876ba2d0b7e017ca34ba638919244eb1f142";
const RELAY =
  "/ip4/192.0.2.10/tcp/3540/webrtc/http/p2p/12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ";

describe("the build's web node", () => {
  test("from the variables, with defaults for the network", () => {
    expect(
      readBuildConfig({
        MINAWEB_RELAYS: RELAY,
        MINAWEB_WASM_URL: "/assets/webnode/pkg/x.wasm",
        MINAWEB_WASM_SHA256: SHA.toUpperCase(),
        MINAWEB_WASM_BYTES: "37785897",
        MINAWEB_START_BYTES: "6754551",
        MINAWEB_COMMIT: "abc",
      }),
    ).toEqual({
      network: "devnet",
      relays: [RELAY],
      seedUrls: ["/webnode-devnet/peers.txt"],
      genesisUrl: "/webnode-devnet/genesis.json",
      iceServersUrls: [],
      frontierStore: true,
      glueUrl: "/assets/webnode/pkg/mina_node_web.js",
      wasm: { url: "/assets/webnode/pkg/x.wasm", sha256: SHA, bytes: 37785897 },
      startBytes: 6754551,
      commit: "abc",
    });
  });
  test("from a build's build-info.json", () => {
    const config = readBuildConfig(
      {},
      {
        network: "mainnet",
        relay: RELAY,
        stamp: "def",
        frontierStore: false,
        wasm: { path: "/assets/webnode/pkg/y.wasm", sha256: SHA, bytes: 5 },
      },
    );
    expect(config).toMatchObject({
      network: "mainnet",
      relays: [RELAY],
      frontierStore: false,
      seedUrls: ["/webnode-mainnet/peers.txt"],
      wasm: { url: "/assets/webnode/pkg/y.wasm", sha256: SHA, bytes: 5 },
      startBytes: null,
      commit: "def",
    });
  });
  test("two relays and each one's ICE servers, from variables or build-info", () => {
    const second = RELAY.replace("192.0.2.10", "198.51.100.20");
    const ice = [
      "https://relay1.example.org/ice-servers.json",
      "https://relay2.example.org/ice-servers.json",
    ];
    const wasm = { MINAWEB_WASM_URL: "/x.wasm", MINAWEB_WASM_SHA256: SHA };
    expect(
      readBuildConfig({
        ...wasm,
        MINAWEB_RELAYS: `${RELAY}, ${second}`,
        MINAWEB_ICE_SERVERS_URL: ice.join(","),
      }),
    ).toMatchObject({ relays: [RELAY, second], iceServersUrls: ice });
    expect(
      readBuildConfig(
        {},
        {
          relays: [RELAY, second],
          iceServers: ice,
          wasm: { path: "/x.wasm", sha256: SHA },
        },
      ),
    ).toMatchObject({ relays: [RELAY, second], iceServersUrls: ice });
    // An older build-info.json: one relay and one list, as strings.
    expect(
      readBuildConfig(
        {},
        {
          relay: RELAY,
          iceServers: "/webnode-devnet/ice-servers.json",
          wasm: { path: "/x.wasm", sha256: SHA },
        },
      ),
    ).toMatchObject({
      relays: [RELAY],
      iceServersUrls: ["/webnode-devnet/ice-servers.json"],
    });
    expect(() =>
      readBuildConfig(
        {},
        {
          relays: [RELAY, "/ip4/1.2.3.4/tcp/1/p2p/x"],
          wasm: { path: "/x.wasm", sha256: SHA },
        },
      ),
    ).toThrow(/WebRTC/);
  });
  test("refuses a build that can't check its WASM, or a bad relay", () => {
    expect(() => readBuildConfig({ MINAWEB_WASM_URL: "/x.wasm" })).toThrow(
      /SHA256/,
    );
    expect(() =>
      readBuildConfig({
        MINAWEB_WASM_URL: "/x.wasm",
        MINAWEB_WASM_SHA256: SHA,
        MINAWEB_RELAYS: "/ip4/1.2.3.4/tcp/8302/p2p/abc",
      }),
    ).toThrow(/WebRTC/);
    expect(() =>
      readBuildConfig({
        MINAWEB_WASM_URL: "/x.wasm",
        MINAWEB_WASM_SHA256: SHA,
        MINAWEB_NETWORK: "berkeley",
      }),
    ).toThrow(/devnet or mainnet/);
  });
});
