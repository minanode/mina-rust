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
  parseBestChainHeaders,
  type BestChainHeaders,
} from "@minanode/node-client";

const header = (height: number) => ({
  height,
  hash: `3N${String(height).padStart(50, "x")}`,
  pred_hash: null,
  timestamp_ms: 1_793_000_000_000,
  user_commands: 0,
  zkapp_commands: 0,
  snark_jobs: 0,
  transaction_fees: "0",
});

/** The node's answer for a query: heights from the tip down to the root,
 * at or below maxHeight. The root moves with `root`. */
let root = 100;
const tip = 160;
function answer(query: {
  limit?: number;
  maxHeight?: number;
}): BestChainHeaders {
  const top = Math.min(tip, query.maxHeight ?? tip);
  const heights: number[] = [];
  for (let h = top; h >= root && heights.length < (query.limit ?? 20); h--) {
    heights.push(h);
  }
  return parseBestChainHeaders({
    k: 290,
    root_height: root,
    best_tip_height: tip,
    headers: heights.map(header),
  });
}

/** The header's best tip, from the shared /status. */
let statusTip: number | null = null;
const refreshStatus = vi.fn();

const polled = <T,>(data: T) => ({
  data,
  error: null,
  updatedAt: 1,
  loading: false,
  refresh: () => {},
});

vi.mock("@minanode/node-client/react", () => ({
  useBestChainHeaders: (query: { limit?: number; maxHeight?: number }) =>
    polled(answer(query)),
  useVerificationOf: () => polled({ receipts: [] }),
  useNodeStatus: () => ({
    status: {
      ...polled(statusTip === null ? null : { bestTip: { height: statusTip } }),
      refresh: refreshStatus,
    },
  }),
  useNodeClient: () => ({ baseUrl: "http://node" }),
  usePoll: () => polled({ receipts: [] }),
}));

const { BestChain, BlocksPage, chainPage, NOT_CHECKED_DOCS_URL } =
  await import("./blocks");

afterEach(cleanup);

test("an older page the root has passed says so and keeps Newest", async () => {
  root = 100;
  const { rerender } = render(<BestChain />);
  // The newest page: 30 blocks, 160 down to 131, the first 10 shown.
  expect(document.querySelectorAll("tr[data-height]")).toHaveLength(10);
  fireEvent.click(screen.getByRole("button", { name: "Show all 30" }));
  expect(document.querySelectorAll("tr[data-height]")).toHaveLength(30);
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Older" })),
  );
  expect(
    document.querySelector("tr[data-height]")?.getAttribute("data-height"),
  ).toBe("130");

  // The root moves past the page: the node answers no headers for it.
  root = 150;
  rerender(<BestChain />);
  expect(document.querySelector("[data-chain-page=expired]")).not.toBeNull();
  expect(screen.queryByText("No best chain yet")).toBeNull();
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Newest" })),
  );
  expect(
    [...document.querySelectorAll("tr[data-height]")].map(r =>
      r.getAttribute("data-height"),
    ),
  ).toEqual([
    "160",
    "159",
    "158",
    "157",
    "156",
    "155",
    "154",
    "153",
    "152",
    "151",
    "150",
  ]);
});

test("a node with no chain yet says so, with no paging", () => {
  root = 200;
  render(<BestChain />);
  expect(document.querySelector("[data-chain-page=empty]")).not.toBeNull();
  expect(screen.queryByRole("button", { name: "Newest" })).toBeNull();
});

test("chainPage tells an expired page from an unsynced node", () => {
  expect(chainPage(null, { headers: [] })).toBe("empty");
  expect(chainPage(130, { headers: [] })).toBe("expired");
  expect(chainPage(130, { headers: [1] })).toBe("list");
});

test("the page links the docs on blocks it did not check", () => {
  render(<BlocksPage />);
  const link = screen.getByRole("link", { name: /Web node docs/ });
  expect(link.getAttribute("href")).toBe(NOT_CHECKED_DOCS_URL);
});

test("a list with a block above the header's tip asks /status again", () => {
  root = 100;
  statusTip = 159;
  refreshStatus.mockClear();
  render(<BestChain />);
  // The list's newest block is 160: the header's 159 is one behind.
  expect(refreshStatus).toHaveBeenCalledTimes(1);
  cleanup();

  statusTip = 160;
  refreshStatus.mockClear();
  render(<BestChain />);
  expect(refreshStatus).not.toHaveBeenCalled();
  statusTip = null;
});
