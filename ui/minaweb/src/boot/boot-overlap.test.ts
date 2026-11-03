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

import { Boot } from "./boot";
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
