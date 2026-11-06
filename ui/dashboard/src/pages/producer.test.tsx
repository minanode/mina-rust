// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { createNodeClient } from "@minanode/node-client";
import {
  NodeClientProvider,
  NodeStatusProvider,
} from "@minanode/node-client/react";

import recorded from "../../../node-client/src/fixtures/producer-attempts-devnet-3610.json";
import { ProducerPage } from "./producer";

afterEach(cleanup);

const KEY = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });

function renderPage({
  producer = recorded as unknown,
  workerOn = false,
  paused = false,
  uptime = 7 * 3600,
} = {}) {
  const fetch = vi.fn(async (url: string | URL | Request) => {
    const path = String(url);
    if (path === "/status")
      return json({
        network: "devnet",
        uptime_secs: uptime,
        block_producer: producer ? KEY : null,
      });
    if (path === "/stats/block_producer") return json(producer);
    if (path === "/snarker/control")
      return json({
        allowed: true,
        enabled: workerOn,
        public_key: workerOn ? KEY : null,
        paused_for_block: paused,
        worker: { kind: workerOn ? "Idle" : "None" },
      });
    if (path === "/build_env") return json({});
    if (path.startsWith("/state/peers")) return json([]);
    return json({ error: "not here" }, 404);
  });
  render(
    <NodeClientProvider client={createNodeClient({ fetch })}>
      <NodeStatusProvider>
        <ProducerPage />
      </NodeStatusProvider>
    </NodeClientProvider>,
  );
}

test("each won slot's outcome, the totals, and why no expected wins", async () => {
  renderPage();
  const slots = await screen.findByRole("heading", { name: "Won slots" });
  const list = slots.closest("section")!;
  expect(within(list).getAllByText("Lost the tie")).toHaveLength(3);
  expect(within(list).getByText("Confirmed")).toBeTruthy();
  expect(within(list).getAllByText("Pending")).toHaveLength(2);
  expect(within(list).getByText("Orphaned")).toBeTruthy();
  expect(within(list).getByText("Scheduled")).toBeTruthy();
  // The canonical block's height links to the explorer.
  expect(
    within(list)
      .getByRole("link", { name: /#579091/ })
      .getAttribute("href"),
  ).toBe(
    "https://o1-labs.github.io/mina-explorer/#/block/579091?network=devnet",
  );

  const outcomes = screen.getByRole("group", { name: "Outcomes" });
  const figure = (label: string) =>
    within(outcomes).getByText(label).nextElementSibling?.textContent;
  expect(figure("Won")).toBe("7");
  expect(figure("Produced")).toBe("4");
  expect(figure("Lost the tie")).toBe("3");

  expect(screen.getByText("Not reported by this node")).toBeTruthy();
  // No worker on: no worker row.
  expect(screen.queryByText("SNARK worker")).toBeNull();
});

test("after a restart, the last run's scheduled slot is labelled as such", async () => {
  // Restarted a minute ago: slot 62780 was scheduled by the run before.
  renderPage({ uptime: 60 });
  const slots = await screen.findByRole("heading", { name: "Won slots" });
  const list = slots.closest("section")!;
  expect(within(list).getAllByText("From before the restart")).toHaveLength(2);
  expect(within(list).queryByText("Scheduled")).toBeNull();
});

test("a won slot's stages start with when it was scheduled, as a clock time", async () => {
  // Slot 62,736 was scheduled at 02:00:01 UTC, 24 min before it began.
  renderPage();
  await screen.findByRole("heading", { name: "Won slots" });
  fireEvent.click(screen.getByRole("button", { name: /slot 62,736$/ }));
  const stages = screen.getByRole("list", {
    name: "Stages of slot 62,736, from the slot's start",
  });
  const first = within(stages).getAllByRole("listitem")[0];
  expect(first.textContent).toBe("Scheduled02:00:01 UTC");
  expect(within(stages).queryByText(/Won \(VRF\)|before/)).toBeNull();
});

test("a slot scheduled the day before shows the date too", async () => {
  const canonical = recorded.attempts.find(
    a => a.won_slot.global_slot === 62736,
  )!;
  // 2026-10-23 22:00:00 UTC.
  const dayBefore = {
    ...canonical,
    times: { ...canonical.times, scheduled: 1_792_792_800_000_000_000 },
  };
  renderPage({ producer: { ...recorded, attempts: [dayBefore] } });
  await screen.findByRole("heading", { name: "Won slots" });
  fireEvent.click(screen.getByRole("button", { name: /slot 62,736$/ }));
  const stages = screen.getByRole("list", {
    name: "Stages of slot 62,736, from the slot's start",
  });
  expect(within(stages).getAllByRole("listitem")[0].textContent).toBe(
    "Scheduled2026-10-23 22:00:00 UTC",
  );
});

test("expected wins from a stake past 2^53 nanomina, in whole MINA", async () => {
  const total = 1_200_000_000_000_000_000;
  renderPage({
    producer: {
      ...recorded,
      vrf_stats: {
        "8": {
          total_slots: 1569,
          evaluated_slots: 1569,
          won_slots: 88,
          delegated_stake: 56_400_000_000_000_000,
          total_currency: total,
          delegators: 5,
        },
        "7": {
          total_slots: 20,
          evaluated_slots: 20,
          won_slots: 1,
          delegated_stake: 56_400_000_000_000_000,
          total_currency: total,
          delegators: 5,
        },
      },
    },
  });
  // The table (from 640 px; a phone has a panel an epoch).
  const table = await screen.findByRole("region", {
    name: "Expected against found table",
  });
  expect(within(table).getAllByText("56,400,000")).toHaveLength(2);
  expect(within(table).getByText("95% range: 81 to 118")).toBeTruthy();
  expect(
    within(table).getByText("within the range; too few to judge"),
  ).toBeTruthy();
  // How the range is worked out is in the docs the header links, not on
  // the page.
  expect(
    within(table).queryByRole("link", { name: /How the range is worked out/ }),
  ).toBeNull();
  expect(screen.queryByText(/binomial/)).toBeNull();
});

test("a worker paused for our block says so, with its page", async () => {
  renderPage({ workerOn: true, paused: true });
  expect(
    await screen.findByText("Paused: a block of ours is due"),
  ).toBeTruthy();
  expect(
    screen.getByRole("link", { name: "Worker" }).getAttribute("href"),
  ).toBe("/snark-worker");
});

test("a node without a producer says how to turn it on", async () => {
  renderPage({ producer: null });
  expect(await screen.findByText("Block production is off")).toBeTruthy();
});

test("a stale restored schedule does not add a missed slot after a reattempt", async () => {
  // The live failure had a restored schedule followed by an orphaned attempt
  // for the same slot. Keep coverage for older nodes as well as repaired files.
  const orphaned = recorded.attempts.find(a => a.status === "Orphaned")!;
  const stale = {
    ...orphaned,
    block: null,
    status: "Scheduled",
    times: { ...orphaned.times, scheduled: 1_000_000 },
  };
  renderPage({ producer: { ...recorded, attempts: [stale, orphaned] } });
  const outcomes = await screen.findByRole("group", { name: "Outcomes" });
  const figure = (label: string) =>
    within(outcomes).getByText(label).nextElementSibling?.textContent;
  expect(figure("Won")).toBe("1");
  expect(figure("Orphaned")).toBe("1");
  expect(figure("Missed")).toBe("0");
});

test("a restored slot that elapsed without another attempt explains its missed outcome", async () => {
  const scheduled = recorded.attempts.find(a => a.status === "Scheduled")!;
  renderPage({
    producer: { ...recorded, attempts: [{ ...scheduled, status: "Missed" }] },
  });
  const outcomes = await screen.findByRole("group", { name: "Outcomes" });
  expect(
    within(outcomes).getByText("Missed").nextElementSibling?.textContent,
  ).toBe("1");
  // The row says it in a few words; its stages give the full reason.
  expect(screen.getByText("Not made after a restart")).toBeTruthy();
  const slots = screen
    .getByRole("heading", { name: "Won slots" })
    .closest("section")!;
  expect(within(slots).getByText("Missed")).toBeTruthy();
  expect(within(slots).queryByText("Producing")).toBeNull();
});

test("the header links to the block producer docs in a new tab", async () => {
  renderPage();
  const link = await screen.findByRole("link", {
    name: /Block producer docs/,
  });
  expect(link.getAttribute("href")).toBe(
    "https://minanode.org/docs/block-producer/",
  );
  expect(link.getAttribute("target")).toBe("_blank");
  expect(link.getAttribute("rel")).toContain("noopener");
});
