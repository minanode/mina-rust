import { describe, expect, test } from "vitest";

import {
  blockedRelay,
  checkPage,
  formatBytes,
  noPeerFailure,
  relayEndpoint,
  type PageFacts,
} from "./failures";

const RELAY =
  "/ip4/192.0.2.10/tcp/3540/webrtc/http/p2p/12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ";
const SECURE_RELAY =
  "/dns4/relay.example.org/tcp/443/webrtc/https/p2p/12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ";

const good: PageFacts = {
  knowsIsolation: true,
  crossOriginIsolated: true,
  isSecureContext: true,
  origin: "https://minaweb.example.org",
  hasWebAssembly: true,
  hasSharedMemory: true,
  hasWebRtc: true,
};

describe("the page check names the real cause", () => {
  test("an isolated page with threads passes", () => {
    expect(checkPage(good)).toBeNull();
  });
  test("plain HTTP on another host: HTTPS or localhost", () => {
    const failure = checkPage({
      ...good,
      isSecureContext: false,
      crossOriginIsolated: false,
      origin: "http://192.0.2.20:4213",
    });
    expect(failure).toMatchObject({
      kind: "isolation",
      title: "This page needs HTTPS",
    });
    expect(failure?.detail).toContain("http://192.0.2.20:4213");
  });
  test("a secure page without the headers: the server's", () => {
    expect(checkPage({ ...good, crossOriginIsolated: false })).toMatchObject({
      kind: "isolation",
      title: "The server didn't isolate this page",
    });
  });
  test("no shared memory: the browser's", () => {
    expect(checkPage({ ...good, hasSharedMemory: false })).toMatchObject({
      kind: "threads",
    });
  });
  test("no WebAssembly: the settings that turn it off", () => {
    const failure = checkPage({
      ...good,
      hasWebAssembly: false,
      hasSharedMemory: false,
    });
    expect(failure).toMatchObject({
      kind: "threads",
      title: "WebAssembly is off in this browser",
    });
    expect(failure?.detail).toContain("Lockdown Mode");
  });
  test("a browser without isolation: the browser's, not the server's", () => {
    expect(
      checkPage({
        ...good,
        knowsIsolation: false,
        crossOriginIsolated: false,
        hasSharedMemory: false,
      }),
    ).toMatchObject({
      kind: "isolation",
      title: "This browser is too old to run the node",
    });
  });
  test("no WebRTC: a setting or an extension", () => {
    expect(checkPage({ ...good, hasWebRtc: false })).toMatchObject({
      kind: "webrtc",
      title: "WebRTC is off in this browser",
    });
  });
});

describe("relays", () => {
  test("the endpoint of a WebRTC multiaddr", () => {
    expect(relayEndpoint(RELAY)).toEqual({
      host: "192.0.2.10:3540",
      secure: false,
    });
    expect(relayEndpoint(SECURE_RELAY)).toEqual({
      host: "relay.example.org:443",
      secure: true,
    });
    expect(relayEndpoint("/ip4/1.2.3.4/tcp/1/p2p/x")).toBeNull();
  });
  test("an HTTPS page can't signal over plain HTTP, except locally", () => {
    expect(blockedRelay(RELAY, "https://minaweb.example.org")).toContain(
      "plain HTTP",
    );
    expect(blockedRelay(RELAY, "http://localhost:4213")).toBeNull();
    expect(blockedRelay(SECURE_RELAY, "https://minaweb.example.org")).toBe(
      null,
    );
    expect(
      blockedRelay(
        RELAY.replace("192.0.2.10", "127.0.0.1"),
        "https://minaweb.example.org",
      ),
    ).toBeNull();
  });
  test("no peer: what was tried and how it went", () => {
    const failure = noPeerFailure([RELAY], "http://localhost:4213", [
      { address: RELAY, status: "Disconnected" },
    ]);
    expect(failure.title).toBe("No peer reachable");
    expect(failure.detail).toBe("No connection to 192.0.2.10:3540.");
    expect(
      noPeerFailure([RELAY], "https://minaweb.example.org", []).detail,
    ).toContain("blocks on an HTTPS page");
    expect(noPeerFailure([], "https://x.example", []).title).toBe(
      "No peer to connect to",
    );
  });
});

test("sizes", () => {
  expect(formatBytes(37_785_897)).toBe("37.8 MB");
  expect(formatBytes(950)).toBe("950 B");
  expect(formatBytes(null)).toBe("–");
});
