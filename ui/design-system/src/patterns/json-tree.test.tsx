import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { foldSummary, jsonText, JsonTree } from "./json-tree";

afterEach(cleanup);

const status = {
  network: "devnet",
  transition_frontier: {
    best_tip: {
      height: 578883,
      hash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
    },
    sync: { phase: "Synced", target: null },
  },
  peers: [{ peer_id: "2aTKv6vap", connected: true }],
  block_producer: null,
};

const button = (name: RegExp) => screen.getByRole("button", { name });

test("summaries and the copied text", () => {
  expect(foldSummary({ a: 1, b: 2 })).toBe("{2 keys}");
  expect(foldSummary({ a: 1 })).toBe("{1 key}");
  expect(foldSummary([1, 2, 3])).toBe("[3 items]");
  expect(jsonText(status)).toBe(JSON.stringify(status, null, 2));
});

test("the root and top-level sections start open, deeper ones folded", () => {
  render(<JsonTree value={status} label="Node status" />);
  expect(button(/root\s*object, 4 keys/)).toHaveProperty(
    "ariaExpanded",
    "true",
  );
  expect(
    button(/^"transition_frontier":\s*\{\s*object, 2 keys/),
  ).toHaveProperty("ariaExpanded", "true");
  expect(button(/^"peers":\s*\[\s*array, 1 item/)).toHaveProperty(
    "ariaExpanded",
    "true",
  );
  // Third level: folded, with its summary shown.
  expect(button(/^"best_tip":\s*\{2 keys\}\s*object, 2 keys/)).toHaveProperty(
    "ariaExpanded",
    "false",
  );
  expect(screen.queryByText(/578883/)).toBeNull();
  expect(screen.getByText('"devnet"')).toBeTruthy();
  // block_producer; sync.target is inside the folded third level.
  expect(screen.getAllByText("null")).toHaveLength(1);
});

test("folds open and shut by click, and keep their state as the value changes", () => {
  const { rerender } = render(<JsonTree value={status} />);
  fireEvent.click(button(/^"best_tip"/));
  expect(screen.getByText("578883")).toBeTruthy();
  fireEvent.click(button(/^"transition_frontier"/));
  expect(button(/^"transition_frontier"/)).toHaveProperty(
    "ariaExpanded",
    "false",
  );
  expect(screen.queryByText("578883")).toBeNull();
  fireEvent.click(button(/^"transition_frontier"/));
  expect(screen.getByText("578883")).toBeTruthy();
  // A refreshed answer: the opened fold stays open, with the new value.
  const next = structuredClone(status);
  next.transition_frontier.best_tip.height = 578884;
  rerender(<JsonTree value={next} />);
  expect(screen.getByText("578884")).toBeTruthy();
});

test("expand all and collapse all", () => {
  render(<JsonTree value={status} />);
  fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
  expect(screen.getByText("578883")).toBeTruthy();
  expect(screen.getByText('"Synced"')).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
  expect(button(/root\s*object/)).toHaveProperty("ariaExpanded", "true");
  expect(button(/^"transition_frontier"/)).toHaveProperty(
    "ariaExpanded",
    "false",
  );
});

test("copy gives the whole value as pretty JSON", async () => {
  const writeText = vi.fn(async () => {});
  Object.defineProperty(window, "isSecureContext", {
    value: true,
    configurable: true,
  });
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText },
    configurable: true,
  });
  render(<JsonTree value={status} label="Node status" />);
  fireEvent.click(
    screen.getByRole("button", { name: /^Copy JSON\s*of Node status$/ }),
  );
  await waitFor(() =>
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(status, null, 2)),
  );
  await screen.findByRole("button", { name: /^Copied\s*of Node status$/ });
});

test("a large array shows a page at a time", () => {
  const items = Array.from({ length: 1000 }, (_, i) => ({ n: i }));
  render(<JsonTree value={{ items }} pageSize={50} />);
  expect(button(/^"items":\s*\[\s*array, 1000 items/)).toHaveProperty(
    "ariaExpanded",
    "true",
  );
  expect(
    screen.getAllByRole("button", {
      name: /^\d+:\s*\{1 key\}\s*object, 1 key/,
    }),
  ).toHaveLength(50);
  fireEvent.click(
    screen.getByRole("button", { name: /Show 50 more \(950 hidden\)/ }),
  );
  expect(
    screen.getAllByRole("button", {
      name: /^\d+:\s*\{1 key\}\s*object, 1 key/,
    }),
  ).toHaveLength(100);
  expect(
    screen.getByRole("button", { name: /Show 50 more \(900 hidden\)/ }),
  ).toBeTruthy();
});

test("empty objects and arrays, and a primitive root", () => {
  render(<JsonTree value={{ empty: {}, none: [] }} />);
  expect(button(/^"empty":\s*\{\}\s*object, 0 keys/)).toHaveProperty(
    "disabled",
    true,
  );
  expect(button(/^"none":\s*\[\]\s*array, 0 items/)).toHaveProperty(
    "disabled",
    true,
  );
  cleanup();
  render(<JsonTree value={"just text"} />);
  expect(screen.getByText('"just text"')).toBeTruthy();
});
