// @vitest-environment jsdom
import * as React from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { createNodeClient } from "@minanode/node-client";
import { NodeClientProvider } from "@minanode/node-client/react";

import { CrashNotice } from "./overview";
import { ReportPage } from "./report";

afterEach(cleanup);

const answer = (complete = true) => ({
  generated_at: new Date().toISOString(),
  title: "Error: BlockApplyFailed [fp:1a2b3c4d]",
  fingerprint: "1a2b3c4d",
  markdown: "## What happened\n\nBlockApplyFailed at <ip>",
  include_network_ids: false,
  issue: {
    repo: "minanode/mina-rust",
    url: "https://github.com/minanode/mina-rust/issues/new?title=t&body=b",
    complete,
  },
  file_name: "report-20261023T231000Z.md",
});

function renderWith(
  page: React.ReactNode,
  respond: (path: string, body: unknown) => Response,
) {
  const calls: { path: string; body: unknown; headers: Headers }[] = [];
  const fetch = vi.fn(
    async (url: string | URL | Request, init?: RequestInit) => {
      const path = String(url);
      const body = init?.body ? JSON.parse(String(init.body)) : null;
      calls.push({ path, body, headers: new Headers(init?.headers) });
      return respond(path, body);
    },
  );
  render(
    <NodeClientProvider client={createNodeClient({ fetch })}>
      {page}
    </NodeClientProvider>,
  );
  return calls;
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });

test("nothing is collected before Prepare report", async () => {
  const calls = renderWith(<ReportPage />, () => json(answer()));
  expect(
    screen.getByRole("heading", { level: 1, name: "Report a problem" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("link", { name: /What a report holds/ }),
  ).toBeTruthy();
  expect(screen.getByText(/mina triage/)).toBeTruthy();
  const ids = screen.getByRole("checkbox", {
    name: "Keep IP addresses and peer ids in the report",
  }) as HTMLInputElement;
  expect(ids.checked).toBe(false);
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(calls).toHaveLength(0);
});

test("a prepared report: copy, download and the issue link", async () => {
  const calls = renderWith(<ReportPage />, () => json(answer()));
  fireEvent.change(screen.getByRole("textbox", { name: "What happened?" }), {
    target: { value: "stops after an hour" },
  });
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByRole("heading", { name: /BlockApplyFailed/ });
  expect(calls).toHaveLength(1);
  expect(calls[0].path).toBe("/report");
  expect(calls[0].headers.get("x-mina-report")).toBe("1");
  expect(calls[0].body).toEqual({
    include_network_ids: false,
    description: "stops after an hour",
  });
  expect(
    screen.getByRole("region", { name: "The report" }).textContent,
  ).toContain("BlockApplyFailed at <ip>");
  expect(screen.getByRole("button", { name: /^Copy/ })).toBeTruthy();
  const issue = screen.getByRole("link", { name: /Open GitHub issue/ });
  expect(issue.getAttribute("href")).toBe(answer().issue.url);
  expect(issue.getAttribute("target")).toBe("_blank");
  expect(issue.getAttribute("rel")).toContain("noopener");
  expect(screen.queryByText("Too long for an issue link")).toBeNull();

  // Download: a .md file named by the node, holding the title and report.
  const created: Blob[] = [];
  URL.createObjectURL = vi.fn((blob: Blob) => {
    created.push(blob);
    return "blob:report";
  });
  URL.revokeObjectURL = vi.fn();
  const clicked: HTMLAnchorElement[] = [];
  const click = vi
    .spyOn(HTMLAnchorElement.prototype, "click")
    .mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this);
    });
  fireEvent.click(screen.getByRole("button", { name: "Download (.md)" }));
  expect(clicked[0].download).toBe("report-20261023T231000Z.md");
  expect(await created[0].text()).toBe(
    "# Error: BlockApplyFailed [fp:1a2b3c4d]\n\n## What happened\n\nBlockApplyFailed at <ip>",
  );
  click.mockRestore();

  // Changing an option drops the report, which no longer matches it.
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Keep IP addresses and peer ids in the report",
    }),
  );
  expect(screen.queryByRole("region", { name: "The report" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByRole("region", { name: "The report" });
  expect(calls[1].body).toMatchObject({ include_network_ids: true });
});

test("a report too long for a link says to attach the download", async () => {
  renderWith(<ReportPage />, () => json(answer(false)));
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByText("Too long for an issue link");
  expect(
    screen.getByText(/attach the file to the issue, or paste it/),
  ).toBeTruthy();
});

test("a refusal or an older node says why, and what to run", async () => {
  renderWith(<ReportPage />, () =>
    json({ error: "the bug report from this page's origin is refused" }, 403),
  );
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByText("No report");
  expect(screen.getByText(/origin is refused/)).toBeTruthy();
  cleanup();
  renderWith(<ReportPage />, () => new Response("", { status: 405 }));
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByText("No report");
  expect(screen.getAllByText(/mina report/).length).toBeGreaterThan(0);
  cleanup();
  renderWith(<ReportPage />, () => new Response("", { status: 404 }));
  fireEvent.click(screen.getByRole("button", { name: /Prepare report/ }));
  await screen.findByText(/older than this dashboard/);
});

test("the overview's crash notice: shown for a crash, quiet otherwise", async () => {
  const calls = renderWith(<CrashNotice />, () =>
    json({
      crash: {
        crashed_ms: Date.UTC(2026, 9, 23, 20, 5),
        message: "frontier root missing at <ip>",
        this_run: false,
        started_ms: Date.UTC(2026, 9, 23, 20, 6),
      },
    }),
  );
  await screen.findByText("The node crashed at 2026-10-23 20:05 UTC");
  expect(screen.getByText(/It restarted after the panic/)).toBeTruthy();
  expect(screen.getByText("frontier root missing at <ip>")).toBeTruthy();
  expect(
    screen.getByRole("link", { name: /Report a problem/ }).getAttribute("href"),
  ).toBe("/report-a-problem");
  expect(calls[0].path).toBe("/report/crash");
  expect(calls[0].headers.get("x-mina-report")).toBe("1");
  cleanup();

  for (const response of [
    () => json({ crash: null }),
    () => new Response("", { status: 405 }),
  ]) {
    const quiet = renderWith(<CrashNotice />, response);
    await waitFor(() => expect(quiet).toHaveLength(1));
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(screen.queryByText(/The node crashed/)).toBeNull();
    cleanup();
  }
});
