// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";

vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  takeHandoff: async () => null,
}));
// jsdom isn't cross-origin isolated: let the boot past its check.
vi.mock("./failures", async original => ({
  ...(await original<typeof import("./failures")>()),
  checkPage: () => null,
}));

import { Boot, CONNECT_FAST_MS, PEER_WAIT_MS, pollDelay } from "./boot";
import type { BuildConfig } from "./build-config";

const CONFIG: BuildConfig = {
  network: "devnet",
  relays: [],
  seedUrls: [],
  genesisUrl: "/genesis.json",
  iceServersUrls: ["https://relay.example/ice-servers.json"],
  frontierStore: false,
  glueUrl: "/glue.js",
  wasm: { url: "/node.wasm", sha256: "00", bytes: null, compressedBytes: null },
  startBytes: null,
  commit: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

test("the start asks for the WASM, the genesis config and the ICE lists with the glue", async () => {
  const asked: string[] = [];
  let downloadStopped = false;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = new URL(input, location.href);
      const request = `${init?.method ?? "GET"} ${url.host === location.host ? url.pathname : url.href}`;
      asked.push(request);
      // The glue isn't there: its import fails, and the boot asks why.
      if (request === "HEAD /glue.js") {
        return Promise.resolve(new Response(null, { status: 404 }));
      }
      if (request === "GET /node.wasm") {
        init?.signal?.addEventListener("abort", () => {
          downloadStopped = true;
        });
      }
      return new Promise<Response>(() => {});
    }),
  );
  const boot = new Boot(CONFIG);
  boot.start();
  await vi.waitFor(() => expect(boot.getSnapshot().failure).not.toBeNull());

  expect(boot.getSnapshot().failure?.kind).toBe("files");
  expect(asked).toEqual(
    expect.arrayContaining([
      "GET /node.wasm",
      "HEAD /genesis.json",
      "GET https://relay.example/ice-servers.json",
      "HEAD /glue.js",
    ]),
  );
  // The glue failed first: its failure shows, and the WASM stops.
  expect(downloadStopped).toBe(true);
});

test("the connect poll is fast while a peer is due, then backs off", () => {
  expect(pollDelay(false, 0)).toBe(200);
  expect(pollDelay(false, CONNECT_FAST_MS - 1)).toBe(200);
  expect(pollDelay(false, CONNECT_FAST_MS)).toBe(1_000);
  expect(pollDelay(false, PEER_WAIT_MS + 59_999)).toBe(1_000);
  expect(pollDelay(false, PEER_WAIT_MS + 60_000)).toBe(5_000);
  expect(pollDelay(true, 0)).toBe(1_000);
  expect(pollDelay(true, 10 * 60_000)).toBe(1_000);
});

test("a node that never gets a peer is asked less and less often", async () => {
  vi.useFakeTimers();
  try {
    const boot = new Boot(CONFIG);
    const status = vi
      .fn()
      .mockResolvedValue({ peers: [], phase: "Connecting" });
    const following = (
      boot as unknown as { follow(c: unknown): Promise<void> }
    ).follow({ status });
    await vi.advanceTimersByTimeAsync(60_000);
    // 50 polls in the first 10 s, then one a second.
    expect(status.mock.calls.length).toBeGreaterThanOrEqual(95);
    expect(status.mock.calls.length).toBeLessThanOrEqual(101);
    const atOneMinute = status.mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    // One a second until 105 s, then one every 5 s.
    expect(status.mock.calls.length - atOneMinute).toBeLessThanOrEqual(50);
    const atTwoMinutes = status.mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(status.mock.calls.length - atTwoMinutes).toBe(12);
    // A peer at last: the boot moves on, then stops at Synced.
    status.mockResolvedValue({ peers: [{ connected: true }], phase: "Synced" });
    await vi.advanceTimersByTimeAsync(5_000);
    await vi.advanceTimersByTimeAsync(1_000);
    await following;
    expect(boot.getSnapshot().synced).toBe(true);
  } finally {
    vi.useRealTimers();
  }
});
