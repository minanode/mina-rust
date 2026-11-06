// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import {
  STATUS_STALE_MS,
  syncView,
  type NodeStatus,
} from "@minanode/node-client";

import { HandoffError } from "../roles/handoff";
import type { HeldKey } from "../roles/keys";

const NOW = 1_793_643_210_000;
const MINUTE = 60_000;

const status = {
  phase: "Synced",
  bestTip: { height: 583745, timestampMs: NOW - 4 * MINUTE },
  target: null,
} as unknown as NodeStatus;

/** The view at NOW, from an answer `sampleAgeMs` old. */
const view = (sampleAgeMs: number, unreachable = false) =>
  syncView(
    {
      phase: "Synced",
      bestTipHeight: 583745,
      bestTipTimestampMs: NOW - 4 * MINUTE,
      targetHeight: null,
      highestPeerTip: 583745,
      sampledAt: NOW - sampleAgeMs,
      unreachable,
    },
    NOW,
  );

let current = view(0);

vi.mock("@minanode/node-client/react", () => ({
  useNodeStatus: () => ({ status: { data: status } }),
  useSyncView: () => current,
}));

vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("./boot-context", () => ({
  useBoot: () => ({ role: { kind: "viewer" } }),
}));

// Shapes only: not real keys.
const held: HeldKey = {
  publicKey: `B62q${"1".repeat(51)}`,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "generated",
};
let keys: { key: HeldKey | null; saved: null } = { key: null, saved: null };
vi.mock("../roles/hooks", () => ({ useKeys: () => keys }));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  handoffSupported: () => true,
}));
const reloadAndStart = vi.fn();
vi.mock("../boot/autostart", () => ({ reloadAndStart }));
const restartWithKey = vi.fn(async (..._args: unknown[]) => {});
vi.mock("../roles/restart", async original => ({
  ...(await original<typeof import("../roles/restart")>()),
  restartWithKey,
}));

const { chipView, OfflineBanner, StatusChip } = await import("./shell");

afterEach(() => {
  cleanup();
  keys = { key: null, saved: null };
  reloadAndStart.mockClear();
  restartWithKey.mockReset();
  current = view(0);
});

test("a fresh answer: the phase, its tip and the block's age", () => {
  expect(chipView(view(1_000), status)).toEqual({
    tone: "ok",
    label: "Synced",
    height: "#583745",
    note: "block 4 min old",
  });
});

test("an answer older than the stale limit isn't shown as Synced", () => {
  const chip = chipView(view(20 * MINUTE), status);
  expect(chip.tone).toBe("warn");
  expect(chip.label).toBe("Stale");
  expect(chip.height).toBe("#583745");
  expect(chip.note).toBe("as of 20 min ago");
  // At the limit it is still the node's status.
  expect(chipView(view(STATUS_STALE_MS), status).label).toBe("Synced");
});

test("no answer at all stays No answer", () => {
  const chip = chipView(view(20 * MINUTE, true), status);
  expect(chip.label).toBe("No answer");
  expect(chip.tone).toBe("error");
});

test("the header chip says Stale over an old answer", () => {
  current = view(20 * MINUTE);
  const { container } = render(<StatusChip />);
  const chip = container.querySelector("[data-status-chip]");
  expect(chip?.getAttribute("data-status-chip")).toBe("stale");
  expect(chip?.textContent).toContain("Stale");
  expect(chip?.textContent).not.toContain("Synced");
  expect(chip?.textContent).toContain("as of 20 min ago");
  current = view(0);
});

const reload = () => screen.getByRole("button", { name: "Reload the page" });

test("no answer, no key held: Reload reloads the page", () => {
  current = view(20 * MINUTE, true);
  render(<OfflineBanner />);
  fireEvent.click(reload());
  expect(reloadAndStart).toHaveBeenCalledOnce();
  expect(document.querySelector("[data-restart-confirm]")).toBeNull();
});

test("no answer, a key held: Reload asks first and keeps the key", async () => {
  current = view(20 * MINUTE, true);
  keys = { key: held, saved: null };
  render(<OfflineBanner />);
  fireEvent.click(reload());
  // Nothing reloads yet: the restart's confirm takes the key across.
  expect(reloadAndStart).not.toHaveBeenCalled();
  expect(document.querySelector("[data-restart-confirm]")).not.toBeNull();
  await act(async () => fireEvent.click(reload()));
  expect(restartWithKey).toHaveBeenCalledWith({ kind: "viewer" }, held, null);
  expect(reloadAndStart).not.toHaveBeenCalled();
});

test("no answer, the key can't cross: the choice, no reload", async () => {
  current = view(20 * MINUTE, true);
  keys = { key: held, saved: null };
  restartWithKey.mockImplementation(async () => {
    throw new HandoffError("This browser couldn't.");
  });
  render(<OfflineBanner />);
  fireEvent.click(reload());
  await act(async () => fireEvent.click(reload()));
  expect(document.querySelector("[data-key-at-risk]")).not.toBeNull();
  expect(reloadAndStart).not.toHaveBeenCalled();
});
