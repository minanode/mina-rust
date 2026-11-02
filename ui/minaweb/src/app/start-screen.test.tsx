// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import type { BuildConfig } from "../boot/build-config";

const R1 = "12D3KooWC4k4X9Ub9JBbLWov3UCH1q4MH7iVEZM2dVfx1dhnYSMo";
const R2 = "12D3KooWN3uRLCr1uZHCVQ11CdvYdXTsQinGQ8JDMah3MZy8M8Q7";
const config = vi.hoisted((): BuildConfig => ({
  network: "devnet",
  relays: [],
  seedUrls: [],
  genesisUrl: "/genesis.json",
  iceServersUrls: [],
  frontierStore: false,
  glueUrl: "/glue.js",
  wasm: {
    url: "/node.wasm",
    sha256: "00",
    bytes: 39_138_241,
    compressedBytes: 5_904_563,
  },
  startBytes: null,
  commit: null,
}));
config.relays = [
  `/dns4/relay1.minaweb.org/tcp/443/webrtc/https/p2p/${R1}`,
  `/dns4/relay2.minaweb.org/tcp/443/webrtc/https/p2p/${R2}`,
];
vi.mock("../config", () => ({ CONFIG: config }));

const { StartScreen } = await import("./start-screen");
const { BootSteps } = await import("./boot-steps");
const { Boot } = await import("../boot/boot");
const { pendingSteps } = await import("../boot/steps");
const { VIEWER } = await import("../roles/role");

afterEach(cleanup);

const LABELS = [
  "Check the browser",
  "Download the node",
  "Compile and start the node",
  "Connect to a peer",
  "Sync the ledgers",
  "Sync the blocks",
];

const items = () =>
  within(screen.getByRole("list", { name: "Starting the web node" }))
    .getAllByRole("listitem")
    .map(li => ({ state: li.dataset.state, text: li.textContent }));

test("the start screen shows the six steps waiting, with their figures", () => {
  render(<StartScreen role={VIEWER} onStart={() => {}} />);
  const steps = items();
  expect(steps.map(s => s.state)).toEqual(Array(6).fill("pending"));
  for (const [i, label] of LABELS.entries()) {
    expect(steps[i].text).toContain(label);
  }
  expect(steps[0].text).toContain(
    "Cross-origin isolation, WebAssembly threads",
  );
  expect(steps[1].text).toContain("39.1 MB, about 5.9 MB compressed");
  expect(steps[2].text).toContain("In this tab");
  expect(steps[3].text).toContain("relay1 · relay2");
  expect(steps[4].text).toContain("From the relays");
  expect(steps[5].text).toContain("To the tip");

  const docs = screen.getByRole("link", { name: /How the web node works/ });
  expect(docs.getAttribute("href")).toBe(
    "https://minanode.org/docs/web-node-connections/",
  );
  expect(docs.getAttribute("target")).toBe("_blank");
});

test("without a compressed size or relays, the figures say less", () => {
  const saved = { wasm: config.wasm, relays: config.relays };
  config.wasm = { ...config.wasm, compressedBytes: null };
  config.relays = [];
  try {
    render(<StartScreen role={VIEWER} onStart={() => {}} />);
    const steps = items();
    expect(steps[1].text).toMatch(/39\.1 MB$/);
    expect(steps[3].text).toContain("From the seed list");
  } finally {
    Object.assign(config, saved);
  }
});

test("the boot runs the same steps the start screen shows", () => {
  const boot = new Boot(config).getSnapshot();
  expect(boot.steps).toEqual(pendingSteps());

  // Under way: the first step runs, the rest wait with their figures.
  const running = {
    ...boot,
    steps: boot.steps.map((s, i) =>
      i === 0 ? { ...s, state: "current" as const, startedAt: Date.now() } : s,
    ),
  };
  render(<BootSteps boot={running} />);
  const steps = items();
  expect(steps.map(s => s.state)).toEqual([
    "current",
    ...Array(5).fill("pending"),
  ]);
  for (const [i, label] of LABELS.entries()) {
    expect(steps[i].text).toContain(label);
  }
  expect(steps[3].text).toContain("relay1 · relay2");
});
