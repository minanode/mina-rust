// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import type { HeldKey } from "../roles/keys";

const takeHandoff = vi.hoisted(() => vi.fn(async () => null));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  takeHandoff,
}));

import { keyStore } from "../roles/keys";
import { Boot } from "./boot";
import type { BuildConfig } from "./build-config";

const CONFIG: BuildConfig = {
  network: "devnet",
  relays: [],
  seedUrls: [],
  genesisUrl: "/genesis.json",
  iceServersUrls: [],
  frontierStore: false,
  glueUrl: "/glue.js",
  wasm: { url: "/node.wasm", sha256: "00", bytes: null, compressedBytes: null },
  startBytes: null,
  commit: null,
};

// Shapes only: not a real key.
const key: HeldKey = {
  publicKey: `B62q${"1".repeat(51)}`,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "generated",
};

/** Starts a boot and waits for its first stop: in jsdom the page isn't
 * cross-origin isolated, so the check after the key fails, as wanted. */
async function startUntilStopped(boot: Boot) {
  boot.start();
  await vi.waitFor(() => expect(boot.getSnapshot().failure).not.toBeNull());
}

beforeEach(() => {
  takeHandoff.mockClear();
  keyStore.forget();
});
afterEach(() => vi.useRealTimers());

test("a Start long after the load holds the key the load took, and takes none", async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  const boot = new Boot(CONFIG);
  // The load took the key (autostart.ts loadGate); Start comes 5 min later,
  // past the handoff's own two minutes.
  boot.useCarried(Promise.resolve(key));
  vi.setSystemTime(Date.now() + 5 * 60_000);
  await startUntilStopped(boot);
  expect(takeHandoff).not.toHaveBeenCalled();
  expect(keyStore.getSnapshot().key).toEqual(key);
});

test("a take that failed at the load isn't tried again", async () => {
  const boot = new Boot(CONFIG);
  boot.useCarried(Promise.reject(new Error("IndexedDB refused")));
  await startUntilStopped(boot);
  expect(takeHandoff).not.toHaveBeenCalled();
  expect(keyStore.getSnapshot().key).toBeNull();
});

test("a take still under way at Start is waited for, once", async () => {
  let resolve!: (k: HeldKey | null) => void;
  const boot = new Boot(CONFIG);
  boot.useCarried(new Promise(r => (resolve = r)));
  boot.start();
  await new Promise(r => setTimeout(r, 20));
  expect(boot.getSnapshot().failure).toBeNull();
  resolve(key);
  await vi.waitFor(() => expect(boot.getSnapshot().failure).not.toBeNull());
  expect(takeHandoff).not.toHaveBeenCalled();
  expect(keyStore.getSnapshot().key).toEqual(key);
});

test("without a take at the load, the boot takes the handoff itself", async () => {
  const boot = new Boot(CONFIG);
  await startUntilStopped(boot);
  expect(takeHandoff).toHaveBeenCalledOnce();
});
