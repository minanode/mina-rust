// @vitest-environment jsdom
/** The App's start screen runs the browser check on the real globals
 * (pageFacts), before Start. */
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const startNode = vi.fn();

vi.mock("../config", () => ({
  CONFIG: {
    network: "devnet",
    relays: [],
    seedUrls: [],
    wasm: {
      url: "/node.wasm",
      sha256: "00",
      bytes: null,
      compressedBytes: null,
    },
    commit: null,
  },
}));
vi.mock("./boot-context", () => ({
  autostart: false,
  startNode: () => startNode(),
  useBoot: () => ({
    role: { kind: "viewer" },
    client: null,
    synced: false,
    failure: null,
    steps: [],
  }),
}));

const { App } = await import("./app");

const globals = globalThis as Record<string, unknown>;
const saved = {
  crossOriginIsolated: globals.crossOriginIsolated,
  isSecureContext: globals.isSecureContext,
  RTCPeerConnection: globals.RTCPeerConnection,
};

/** A browser that can run the node: isolated, secure, with WebRTC. Node's
 * own WebAssembly gives the shared memory. */
beforeEach(() => {
  globals.crossOriginIsolated = true;
  globals.isSecureContext = true;
  globals.RTCPeerConnection = function RTCPeerConnection() {};
});
afterEach(() => {
  cleanup();
  for (const [name, value] of Object.entries(saved)) {
    if (value === undefined) delete globals[name];
    else globals[name] = value;
  }
});

const start = () =>
  screen.getByRole("button", {
    name: "Start the web node in this tab",
  }) as HTMLButtonElement;

test("a browser that can run the node gets Start and no notice", () => {
  render(<App />);
  expect(start().disabled).toBe(false);
  expect(screen.queryByRole("note")).toBeNull();
});

test("without RTCPeerConnection, the start screen says WebRTC is off", () => {
  delete globals.RTCPeerConnection;
  render(<App />);
  expect(start().disabled).toBe(true);
  const note = screen.getByRole("note");
  expect(note.dataset.failure).toBe("webrtc");
  expect(note.textContent).toContain("WebRTC is off in this browser");
});

test("a browser that knows no isolation is too old, not misserved", () => {
  delete globals.crossOriginIsolated;
  render(<App />);
  expect(start().disabled).toBe(true);
  expect(screen.getByRole("note").textContent).toContain(
    "This browser is too old to run the node",
  );
});
