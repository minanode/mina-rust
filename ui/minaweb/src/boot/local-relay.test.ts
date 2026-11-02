import { afterEach, expect, test, vi } from "vitest";
import { localRelayConfig, validRelayAddress } from "./local-relay";
import type { BuildConfig } from "./build-config";

const relay =
  "/ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/12D3KooWGdtdRfzSWjTNnCabYQkCqjXiAkcBx9JvpgpfCMFWJsgQ";
const config = {
  relays: ["old relay"],
  seedUrls: ["/old-peers.txt"],
  network: "devnet",
} as BuildConfig;
afterEach(() => vi.unstubAllGlobals());

test("local relay replaces all compiled seed sources", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ relays: [relay] }))),
  );
  expect(await localRelayConfig(config)).toEqual({
    ...config,
    relays: [relay],
    seedUrls: [],
  });
});

test.each([
  {},
  { relays: [] },
  { relays: ["bad"] },
  { relays: [relay, relay] },
])("refuses malformed config %j", async value => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response(JSON.stringify(value))),
  );
  await expect(localRelayConfig(config)).rejects.toThrow(
    "Invalid local relay config",
  );
});

test("missing local config never falls back to compiled seeds", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response("", { status: 404 })),
  );
  await expect(localRelayConfig(config)).rejects.toThrow("did not supply");
});

import cases from "../../../../tools/release/relay-cases.json";
test.each(cases.valid)("accepts relay %s", relay => {
  expect(validRelayAddress(relay)).toBe(true);
});
test.each(cases.invalid)("rejects relay %s", relay => {
  expect(validRelayAddress(relay)).toBe(false);
});
