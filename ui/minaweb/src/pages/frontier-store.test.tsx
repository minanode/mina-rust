// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";

import { afterEach, beforeEach, expect, test, vi } from "vitest";

import type { FrontierStore } from "@minanode/node-client";

const state = vi.hoisted(() => ({
  store: null as FrontierStore | null,
  on: true,
  restore: null as unknown,
}));
vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../app/boot-context", () => ({
  useBoot: () => ({ frontierStore: state.on, frontierStoreReset: false }),
}));
vi.mock("../boot/boot", () => ({ resetFrontierStore: () => true }));
vi.mock("@minanode/node-client/react", () => ({
  useNodeStatus: () => ({
    status: { data: { frontierStore: state.store } },
  }),
  useSyncRestore: () => ({ data: state.restore }),
}));

const { FrontierStoreCard } = await import("./boot");
const { SecondTabBanner } = await import("../app/second-tab-banner");
const { storeLabel, storeState } = await import("../lib/store");

const open: FrontierStore = { open: true, unavailable: null, locked: false };
const opening: FrontierStore = { ...open, open: false };
const locked: FrontierStore = {
  open: false,
  unavailable: "the store is locked by another process",
  locked: true,
};

beforeEach(() => {
  state.store = null;
  state.on = true;
  state.restore = null;
});
afterEach(cleanup);

const card = () =>
  document.querySelector("[data-frontier-store]") as HTMLElement;
const resetButton = () => document.querySelector("[data-action=reset-store]");

test("the store's status, as the Overview and Start-up show it", () => {
  expect(storeLabel(storeState(false, open))).toBe("Off");
  expect(storeLabel(storeState(true, null))).toBe("Opening");
  expect(storeLabel(storeState(true, opening))).toBe("Opening");
  expect(storeLabel(storeState(true, open))).toBe("On");
  expect(storeLabel(storeState(true, locked))).toBe(
    "Not available in this tab",
  );
  expect(storeState(true, { ...opening, unavailable: "disk full" })).toEqual({
    kind: "unavailable",
    reason: "disk full",
  });
});

test("before the node says, or while the store opens: no reset", () => {
  render(<FrontierStoreCard />);
  expect(card().dataset.frontierStore).toBe("opening");
  expect(screen.getByText("Opening")).toBeTruthy();
  expect(resetButton()).toBeNull();
  cleanup();
  state.store = opening;
  render(<FrontierStoreCard />);
  expect(resetButton()).toBeNull();
});

test("open in this tab: On, how this run started, and the reset", () => {
  state.store = open;
  render(<FrontierStoreCard />);
  expect(card().dataset.frontierStore).toBe("on");
  expect(screen.getByText("On")).toBeTruthy();
  expect(screen.getByText("Synced from the network")).toBeTruthy();
  expect(resetButton()).toBeTruthy();
});

test("a second tab: not available, held by another tab, and no reset", () => {
  state.store = locked;
  render(<FrontierStoreCard />);
  expect(card().dataset.frontierStore).toBe("other-tab");
  expect(screen.getByText("Not available in this tab")).toBeTruthy();
  expect(screen.getByText("Another tab")).toBeTruthy();
  expect(screen.queryByText(/A reload starts from them/)).toBeNull();
  expect(resetButton()).toBeNull();
});

test("a second tab gets one warning; a first tab none", () => {
  state.store = open;
  render(<SecondTabBanner />);
  expect(screen.queryByRole("alert")).toBeNull();
  cleanup();
  state.store = locked;
  render(<SecondTabBanner />);
  expect(screen.getByRole("alert").textContent).toContain(
    "Another tab runs this browser's node.",
  );
});

test("another failure to open the store: its reason, no second-tab warning", () => {
  state.store = { ...opening, unavailable: "disk full" };
  render(<FrontierStoreCard />);
  expect(screen.getByText("disk full")).toBeTruthy();
  expect(resetButton()).toBeNull();
  cleanup();
  render(<SecondTabBanner />);
  expect(screen.queryByRole("alert")).toBeNull();
});

test("the store off for this run: Off, and no reset", () => {
  state.on = false;
  render(<FrontierStoreCard />);
  expect(card().dataset.frontierStore).toBe("off");
  expect(screen.getByText("Off")).toBeTruthy();
  expect(resetButton()).toBeNull();
});

test("every docs link on the card opens a new tab", () => {
  state.store = open;
  render(<FrontierStoreCard />);
  const link = screen.getByRole("link", { name: /Web node docs/ });
  expect(link.getAttribute("target")).toBe("_blank");
  expect(link.getAttribute("href")).toMatch(/#saved-chain$/);
  const docs = readFileSync(
    `${import.meta.dirname}/../../../minanode-site/app/docs/web-node-connections/page.tsx`,
    "utf8",
  );
  expect(docs).toContain('id="saved-chain"');
});
