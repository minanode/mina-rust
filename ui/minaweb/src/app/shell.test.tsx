// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import {
  STATUS_STALE_MS,
  syncView,
  type NodeStatus,
} from "@minanode/node-client";

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

const { chipView, StatusChip } = await import("./shell");

afterEach(cleanup);

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
