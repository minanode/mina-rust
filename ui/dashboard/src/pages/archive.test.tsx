// @vitest-environment jsdom
import * as React from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { createNodeClient } from "@minanode/node-client";
import {
  NodeClientProvider,
  NodeStatusProvider,
} from "@minanode/node-client/react";

import { Shell } from "../app/shell";
import { ArchivePage } from "./archive";

afterEach(cleanup);

const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/** A state hash for `height`: 52 base58 characters. */
function hash(height: number): string {
  let body = "";
  let n = height * 7919 + 1;
  while (body.length < 50) {
    body += ALPHABET[n % ALPHABET.length];
    n = Math.floor(n / 3) + body.length * 131;
  }
  return `3N${body}`;
}

const block = (height: number) => ({ height, state_hash: hash(height) });

const TIP = 591_571;

/** `GET /status` of a node at `TIP`, synced unless `phase` says. */
function nodeStatus(phase = "Synced") {
  const now = Date.now();
  return {
    network: "devnet",
    transition_frontier: {
      best_tip: {
        height: TIP,
        hash: hash(TIP),
        timestamp_ms: now - 60_000,
        timestamp: (now - 60_000) * 1_000_000,
      },
      sync: { status: phase, phase, time: now * 1_000_000, target: null },
    },
    peers: [],
  };
}

/** `GET /archive/status` of a feed that is up to date at `TIP`. */
function archiveStatus(overrides: Record<string, unknown> = {}) {
  const now = Date.now();
  return {
    outbox_dir: "/var/lib/mina/archive-outbox",
    archive_address: "127.0.0.1:3086",
    pending_entries: 0,
    pending_bytes: 0,
    retained_entries: 870,
    retained_bytes: 83_000_000,
    total_entries: 870,
    total_bytes: 83_000_000,
    retain_blocks: 870,
    highest_accepted_height: TIP,
    oldest_retained: block(TIP - 869),
    newest_retained: block(TIP),
    oldest_pending: null,
    newest_pending: null,
    max_entries: 20_000,
    max_bytes: 4 << 30,
    feed_started_at: block(580_538),
    delivered: 11_034,
    dropped_full: 0,
    corrupt: 0,
    gaps_total: 0,
    last_delivered: block(TIP),
    recent_gaps: [],
    consecutive_failures: 0,
    last_error: null,
    last_attempt_at_ms: now - 4_000,
    last_success_at_ms: now - 4_000,
    failing_since_ms: null,
    stuck_on: null,
    note: "RPC acknowledgements precede database commits.",
    ...overrides,
  };
}

const retainedEntry = (height: number, seq = height) => ({
  height,
  state_hash: hash(height),
  state: "retained",
  seq,
  size: 95_000,
});

function blocksAnswer(entries: unknown[]) {
  return {
    entries,
    matching: entries.length,
    limit: 20,
    oldest_retained: block(TIP - 869),
    newest_retained: block(TIP),
    retain_blocks: 870,
    highest_accepted_height: TIP,
  };
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });

type Call = { path: string; method: string; body: unknown; headers: Headers };

function renderWith(
  page: React.ReactNode,
  {
    phase = "Synced",
    archive = archiveStatus() as unknown,
    archiveCode = 200,
    blocks = (path: string): Response => {
      void path;
      return json(
        blocksAnswer([TIP, TIP - 1, TIP - 2].map(h => retainedEntry(h))),
      );
    },
    resend = (body: unknown): Response => {
      void body;
      return json({ result: "requeued", requeued: 1, blocks: [block(TIP)] });
    },
  } = {},
) {
  const calls: Call[] = [];
  const fetch = vi.fn(
    async (url: string | URL | Request, init?: RequestInit) => {
      const path = String(url);
      const body = init?.body ? JSON.parse(String(init.body)) : null;
      const method = init?.method ?? "GET";
      calls.push({ path, method, body, headers: new Headers(init?.headers) });
      if (path === "/status") return json(nodeStatus(phase));
      if (path === "/build_env") return json({ version: "0.22.2" });
      if (path === "/archive/status") return json(archive, archiveCode);
      if (path.startsWith("/archive/blocks")) return blocks(path);
      if (path === "/archive/resend") return resend(body);
      return json({}, 404);
    },
  );
  render(
    <NodeClientProvider client={createNodeClient({ fetch })}>
      <NodeStatusProvider>{page}</NodeStatusProvider>
    </NodeClientProvider>,
  );
  return calls;
}

const feed = async (label: string) =>
  await waitFor(() => {
    const signal = document.querySelector("[data-archive-feed]");
    expect(signal?.textContent).toBe(label);
    return signal!;
  });

test("the Archive nav item shows only while /archive/status answers", async () => {
  renderWith(<Shell route={{ page: "overview" }}>page</Shell>);
  const link = await screen.findByRole("link", { name: "Archive" });
  expect(link.getAttribute("href")).toBe("/archive-node");
  cleanup();

  for (const response of [
    () => json({ error: "this node does not feed an archive process" }, 404),
    () => new Response("", { status: 404 }),
  ]) {
    const calls: Call[] = [];
    const fetch = vi.fn(async (url: string | URL | Request) => {
      const path = String(url);
      calls.push({
        path,
        method: "GET",
        body: null,
        headers: new Headers(),
      });
      if (path === "/archive/status") return response();
      if (path === "/status") return json(nodeStatus());
      return json({});
    });
    render(
      <NodeClientProvider client={createNodeClient({ fetch })}>
        <NodeStatusProvider>
          <Shell route={{ page: "overview" }}>page</Shell>
        </NodeStatusProvider>
      </NodeClientProvider>,
    );
    await waitFor(() =>
      expect(calls.some(c => c.path === "/archive/status")).toBe(true),
    );
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(screen.getByRole("link", { name: "SNARK worker" })).toBeTruthy();
    expect(screen.queryByRole("link", { name: "Archive" })).toBeNull();
    cleanup();
  }
});

test("on a node that doesn't feed an archive the page doesn't exist", async () => {
  renderWith(<ArchivePage path="/archive-node" />, {
    archive: { error: "this node does not feed an archive process" },
    archiveCode: 404,
  });
  await screen.findByRole("heading", { level: 1, name: "No such page" });
  expect(screen.queryByText("Archive docs")).toBeNull();
});

test("up to date: the status, the best tip, the values, no gaps", async () => {
  renderWith(<ArchivePage />);
  await feed("Up to date");
  expect(
    screen.getByRole("heading", { level: 1, name: "Archive" }),
  ).toBeTruthy();
  const glance = screen.getByRole("group", { name: "Feed at a glance" });
  expect(within(glance).getByText("Best tip")).toBeTruthy();
  expect(within(glance).getAllByText("#591571")).toHaveLength(2);
  expect(within(glance).getByText("0 blocks")).toBeTruthy();
  expect(screen.getByText("127.0.0.1:3086")).toBeTruthy();
  expect(screen.getByText("#590702 to #591571")).toBeTruthy();
  expect(screen.getByText("83 MB")).toBeTruthy();
  expect(screen.getByText("No gaps")).toBeTruthy();
  // Without the database check, delivered, never "stored": the node can't
  // see the database.
  expect(document.body.textContent).not.toMatch(/stored/i);
  // One docs link.
  const docs = screen.getAllByRole("link", { name: /docs/i });
  expect(docs).toHaveLength(1);
  expect(docs[0].getAttribute("href")).toBe(
    "https://minanode.org/docs/dashboard/#archive-page",
  );
  // The recent delivered blocks, each with a replay.
  await waitFor(() =>
    expect(
      screen.getAllByRole("button", { name: /^Replay block/ }).length,
    ).toBeGreaterThan(0),
  );
});

test("catching up, failing and not synced", async () => {
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      pending_entries: 12,
      pending_bytes: 1_200_000,
      newest_retained: block(TIP - 12),
      last_delivered: block(TIP - 12),
    }),
  });
  await feed("Catching up (12 waiting)");
  expect(screen.getByText("12 blocks")).toBeTruthy();
  cleanup();

  const since = Date.UTC(2026, 10, 18, 17, 20);
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      pending_entries: 3,
      consecutive_failures: 7,
      last_error: "connect to 127.0.0.1:3086: Connection refused",
      failing_since_ms: since,
      stuck_on: block(TIP - 2),
      newest_retained: block(TIP - 3),
      last_delivered: block(TIP - 3),
    }),
  });
  await feed("Delivery failing");
  const alert = document.querySelector("[data-archive-failing]")!;
  expect(alert.textContent).toContain("since 2026-11-18 17:20 UTC");
  expect(alert.textContent).toContain("Connection refused");
  expect(alert.textContent).toContain("7 attempts failed on block #591569");
  cleanup();

  renderWith(<ArchivePage />, { phase: "Bootstrap" });
  await feed("Node not synced");
});

test("gaps since the feed started: kind, heights and hash", async () => {
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      gaps_total: 2,
      recent_gaps: [
        {
          kind: "unseen_parent",
          from_height: 585_001,
          to_height: 585_040,
          parent_hash: hash(585_040),
          first_block_after: block(585_041),
          detected_at_ms: Date.now() - 3_600_000,
        },
        {
          kind: "dropped_full",
          from_height: 589_000,
          to_height: 589_000,
          state_hash: hash(589_000),
          detected_at_ms: Date.now() - 60_000,
        },
      ],
    }),
  });
  await feed("Up to date");
  const table = screen.getByRole("region", { name: "Gaps table" });
  const rows = within(table).getAllByRole("row");
  // Newest first.
  expect(rows[1].textContent).toContain("Dropped, outbox full");
  expect(rows[1].textContent).toContain("#589000");
  expect(rows[2].textContent).toContain("Never seen");
  expect(rows[2].textContent).toContain("#585001 to #585040");
});

test("replay a block: find it, confirm, and the result", async () => {
  const calls = renderWith(<ArchivePage />, {
    blocks: path =>
      json(
        blocksAnswer(
          path.includes("height=591000")
            ? [retainedEntry(591_000), retainedEntry(591_000, 9_999)]
            : [retainedEntry(TIP)],
        ),
      ),
  });
  await feed("Up to date");
  const box = screen.getByRole("textbox", { name: "Height or state hash" });
  fireEvent.change(box, { target: { value: "591,000" } });
  fireEvent.click(screen.getByRole("button", { name: /Find/ }));
  const matches = await screen.findByRole("region", {
    name: "Matching blocks table",
  });
  expect(within(matches).getAllByRole("row")).toHaveLength(3);
  expect(
    calls.some(c => c.path === "/archive/blocks?height=591000&limit=20"),
  ).toBe(true);
  fireEvent.click(
    within(matches).getAllByRole("button", { name: "Replay block #591000" })[0],
  );
  const confirm = screen.getByRole("group", {
    name: "Confirm replaying block #591000",
  });
  expect(confirm.textContent).toContain(
    "Send block #591000 to the archive again?",
  );
  // Nothing is sent before the confirm.
  expect(calls.some(c => c.path === "/archive/resend")).toBe(false);
  fireEvent.click(within(confirm).getByRole("button", { name: "Replay" }));
  await screen.findByText(
    "Block #591000 is queued again, ahead of newer waiting blocks.",
  );
  // A new lookup forgets the last outcome.
  fireEvent.change(box, { target: { value: "591001" } });
  fireEvent.click(screen.getByRole("button", { name: /Find/ }));
  await waitFor(() =>
    expect(
      screen.queryByText(
        "Block #591000 is queued again, ahead of newer waiting blocks.",
      ),
    ).toBeNull(),
  );
  const post = calls.find(c => c.path === "/archive/resend")!;
  expect(post.method).toBe("POST");
  expect(post.headers.get("x-mina-archive-control")).toBe("1");
  expect(post.headers.get("content-type")).toBe("application/json");
  expect(post.body).toEqual({ state_hash: hash(591_000) });
});

test("a block outside the retained window isn't retained by this node", async () => {
  renderWith(<ArchivePage />, {
    blocks: path =>
      json(blocksAnswer(path.includes("height=") ? [] : [retainedEntry(TIP)])),
  });
  await feed("Up to date");
  fireEvent.change(
    screen.getByRole("textbox", { name: "Height or state hash" }),
    { target: { value: "12345" } },
  );
  fireEvent.click(screen.getByRole("button", { name: /Find/ }));
  await screen.findByText("Height #12345 isn't retained by this node");
  expect(
    screen.getByText(/It keeps heights #590702 to #591571\./).textContent,
  ).toContain("precomputed-block import");
  // Still one docs link on the page.
  expect(screen.getAllByRole("link", { name: /docs/i })).toHaveLength(1);

  fireEvent.change(
    screen.getByRole("textbox", { name: "Height or state hash" }),
    { target: { value: "not a block" } },
  );
  fireEvent.click(screen.getByRole("button", { name: /Find/ }));
  await screen.findByText("Enter a block height or a state hash (3N…).");
});

test("a pending block and a refused replay say so", async () => {
  let answer = (): Response =>
    json({
      result: "already_pending",
      requeued: 0,
      blocks: [block(TIP)],
      in_flight: false,
    });
  renderWith(<ArchivePage />, {
    blocks: () => json(blocksAnswer([retainedEntry(TIP)])),
    resend: () => answer(),
  });
  await feed("Up to date");
  const recent = await screen.findByRole("region", {
    name: "Recently delivered blocks table",
  });
  const replay = () => {
    fireEvent.click(
      within(recent).getByRole("button", { name: "Replay block #591571" }),
    );
    fireEvent.click(
      within(
        screen.getByRole("group", { name: "Confirm replaying block #591571" }),
      ).getByRole("button", { name: "Replay" }),
    );
  };
  replay();
  await screen.findByText("Block #591571 is already waiting.");
  answer = () =>
    json(
      {
        error:
          "archive replay from this page's origin is refused: allow it with --worker-control-origin",
      },
      403,
    );
  replay();
  await screen.findByText("The node refused the replay");
  expect(screen.getByText(/origin is refused/)).toBeTruthy();
  expect(screen.getByText(/Open the dashboard at the node's own/)).toBeTruthy();
});

test("resend all retained: how many and how much, behind a confirm", async () => {
  const calls = renderWith(<ArchivePage />, {
    resend: () => json({ result: "requeued", requeued: 870 }),
  });
  await feed("Up to date");
  fireEvent.click(screen.getByRole("button", { name: /Resend all retained/ }));
  const confirm = screen.getByRole("group", {
    name: "Confirm resending every retained block",
  });
  expect(confirm.textContent).toContain(
    "Send all 870 retained blocks (83 MB) to the archive again?",
  );
  expect(calls.some(c => c.path === "/archive/resend")).toBe(false);
  fireEvent.click(within(confirm).getByRole("button", { name: "Resend all" }));
  await screen.findByText("870 blocks queued for the archive again.");
  const post = calls.find(c => c.path === "/archive/resend")!;
  expect(post.body).toEqual({ all: true });
  expect(post.headers.get("x-mina-archive-control")).toBe("1");
});

test("the recent delivered blocks show ten, then all twenty", async () => {
  const calls = renderWith(<ArchivePage />, {
    blocks: () =>
      json(
        blocksAnswer(
          Array.from({ length: 20 }, (_, i) => retainedEntry(TIP - i)),
        ),
      ),
  });
  await feed("Up to date");
  const recent = await screen.findByRole("region", {
    name: "Recently delivered blocks table",
  });
  await waitFor(() =>
    expect(within(recent).getAllByRole("row")).toHaveLength(11),
  );
  expect(
    calls.some(c => c.path === "/archive/blocks?state=retained&limit=20"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Show all 20" }));
  expect(within(recent).getAllByRole("row")).toHaveLength(21);
});

/** The status's `database` section: complete unless `overrides` say. */
function database(overrides: Record<string, unknown> = {}) {
  const now = Date.now();
  return {
    configured: true,
    metrics_url: "http://127.0.0.1:3087/metrics",
    verdict: "complete",
    last_attempt_at_ms: now - 20_000,
    last_success_at_ms: now - 20_000,
    last_error: null,
    max_block_height: TIP,
    missing_blocks: 0,
    unparented_blocks: 0,
    delivered_height: TIP,
    stored_tip_lag: 0,
    expected_stored_height: TIP - 4,
    behind_after_ms: 360_000,
    poll_interval_ms: 60_000,
    ...overrides,
  };
}

const card = async (verdict: string) =>
  await waitFor(() => {
    const el = document.querySelector<HTMLElement>(
      `[data-archive-database="${verdict}"]`,
    );
    expect(el).not.toBeNull();
    return el!;
  });

test("database complete: the archive's counts, the feed up to date", async () => {
  renderWith(<ArchivePage />, {
    archive: archiveStatus({ database: database() }),
  });
  await feed("Up to date");
  const db = await card("complete");
  expect(within(db).getByText("Archive database")).toBeTruthy();
  expect(within(db).getByText("Complete")).toBeTruthy();
  expect(within(db).getByText("Highest stored")).toBeTruthy();
  expect(within(db).getByText("#591571")).toBeTruthy();
  expect(within(db).getByText("same as last delivered")).toBeTruthy();
  expect(within(db).getByText("Missing heights")).toBeTruthy();
  expect(within(db).getByText("recent window")).toBeTruthy();
  expect(within(db).getByText("Blocks without a parent")).toBeTruthy();
  expect(within(db).getAllByText("0")).toHaveLength(2);
  expect(within(db).getByText("Checked")).toBeTruthy();
  expect(within(db).getByText("http://127.0.0.1:3087/metrics")).toBeTruthy();
  expect(db.querySelector("[data-archive-database-action]")).toBeNull();
  // Still one docs link.
  expect(screen.getAllByRole("link", { name: /docs/i })).toHaveLength(1);
});

test("database incomplete: the top status, and resend all for the window", async () => {
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: database({
        verdict: "incomplete",
        missing_blocks: 3,
        unparented_blocks: 2,
        max_block_height: TIP - 1,
        stored_tip_lag: 1,
      }),
    }),
  });
  await feed("Database incomplete");
  const db = await card("incomplete");
  expect(within(db).getByText("Incomplete")).toBeTruthy();
  expect(within(db).getByText("3")).toBeTruthy();
  expect(within(db).getByText("2")).toBeTruthy();
  expect(within(db).getByText("1 block below last delivered")).toBeTruthy();
  const action = db.querySelector("[data-archive-database-action]")!;
  expect(action.getAttribute("data-archive-database-action")).toBe("resend");
  expect(action.textContent).toBe(
    "Resend all retained to refill heights #590702 to #591571. Older heights need a precomputed-block import.",
  );
  // The button it names is on the page.
  expect(
    screen.getByRole("button", { name: "Resend all retained" }),
  ).toBeTruthy();
});

test("database behind and unavailable", async () => {
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: database({
        verdict: "behind",
        max_block_height: TIP - 9,
        stored_tip_lag: 9,
      }),
    }),
  });
  await feed("Database behind");
  let db = await card("behind");
  expect(within(db).getByText("Behind")).toBeTruthy();
  expect(within(db).getByText("9 blocks below last delivered")).toBeTruthy();
  expect(db.querySelector("[data-archive-database-action]")!.textContent).toBe(
    "Check the archive's log and its database connection.",
  );
  cleanup();

  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: database({
        verdict: "unavailable",
        last_attempt_at_ms: Date.now() - 5_000,
        last_error: "cannot connect: Connection refused (os error 111)",
      }),
    }),
  });
  // Unavailable doesn't change the feed's own status.
  await feed("Up to date");
  db = await card("unavailable");
  expect(within(db).getByText("Unavailable")).toBeTruthy();
  expect(within(db).getByText("Last error")).toBeTruthy();
  expect(
    within(db).getByText("cannot connect: Connection refused (os error 111)"),
  ).toBeTruthy();
  // The last good values stay, with when they were read.
  expect(within(db).getByText("#591571")).toBeTruthy();
  expect(db.querySelector("[data-archive-database-action]")!.textContent).toBe(
    "Check the archive runs with --metrics-port at MINA_ARCHIVE_METRICS_ADDRESS.",
  );
});

test("database not checked: one line saying what to set", async () => {
  renderWith(<ArchivePage />);
  await feed("Up to date");
  const db = await card("not_checked");
  expect(within(db).getByText("Not checked")).toBeTruthy();
  expect(within(db).queryByText("Highest stored")).toBeNull();
  expect(db.querySelector("[data-archive-database-action]")!.textContent).toBe(
    "Set MINA_ARCHIVE_METRICS_ADDRESS to the archive's --metrics-port URL.",
  );
  cleanup();

  // Configured, before the first fetch.
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: database({
        verdict: "not_checked",
        last_attempt_at_ms: null,
        last_success_at_ms: null,
        max_block_height: null,
        missing_blocks: null,
        unparented_blocks: null,
        stored_tip_lag: null,
      }),
    }),
  });
  const checking = await card("not_checked");
  expect(within(checking).getByText("Checking")).toBeTruthy();
  expect(checking.querySelector("[data-archive-database-action]")).toBeNull();
});

test("the database check ranks below failing and not synced, above catching up", async () => {
  const incomplete = database({ verdict: "incomplete", missing_blocks: 1 });
  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: incomplete,
      pending_entries: 4,
      newest_retained: block(TIP - 4),
      last_delivered: block(TIP - 4),
    }),
  });
  await feed("Database incomplete");
  cleanup();

  renderWith(<ArchivePage />, {
    archive: archiveStatus({
      database: incomplete,
      consecutive_failures: 2,
      last_error: "refused",
    }),
  });
  await feed("Delivery failing");
  cleanup();

  renderWith(<ArchivePage />, {
    phase: "Bootstrap",
    archive: archiveStatus({ database: incomplete }),
  });
  await feed("Node not synced");
});
