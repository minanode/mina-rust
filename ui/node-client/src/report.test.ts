import { expect, test, vi } from "vitest";

import { createNodeClient } from "./client";
import {
  parseCrashNotice,
  parseReport,
  REPORT_HEADER,
  reportDocument,
} from "./report";

const answer = {
  generated_at: "2026-10-23T23:10:00Z",
  title: "Error: BlockApplyFailed [fp:1a2b3c4d]",
  fingerprint: "1a2b3c4d",
  markdown: "## What happened\n\n...",
  include_network_ids: false,
  issue: {
    repo: "minanode/mina-rust",
    url: "https://github.com/minanode/mina-rust/issues/new?title=x&body=y",
    complete: false,
  },
  file_name: "report-20261023T231000Z.md",
};

test("a report as the node sends it", () => {
  const report = parseReport(answer);
  expect(report).toMatchObject({
    title: "Error: BlockApplyFailed [fp:1a2b3c4d]",
    includeNetworkIds: false,
    issue: { repo: "minanode/mina-rust", complete: false },
    fileName: "report-20261023T231000Z.md",
  });
  expect(report.generatedMs).toBe(Date.parse("2026-10-23T23:10:00Z"));
  expect(reportDocument(report)).toBe(
    "# Error: BlockApplyFailed [fp:1a2b3c4d]\n\n## What happened\n\n...",
  );
});

test("the crash notice, or none", () => {
  expect(parseCrashNotice({ crash: null })).toBeNull();
  expect(parseCrashNotice({})).toBeNull();
  expect(
    parseCrashNotice({
      crash: {
        crashed_ms: 1_792_800_000_000,
        message: "frontier root missing at <ip>",
        this_run: false,
        started_ms: 1_792_800_060_000,
      },
    }),
  ).toEqual({
    crashedMs: 1_792_800_000_000,
    message: "frontier root missing at <ip>",
    thisRun: false,
    startedMs: 1_792_800_060_000,
  });
});

test("report requests carry the header and say only what they ask", async () => {
  const fetch = vi.fn(async (url: string | URL | Request) =>
    String(url).endsWith("/crash")
      ? new Response('{"crash": null}')
      : new Response(JSON.stringify(answer)),
  );
  const client = createNodeClient({ baseUrl: "http://127.0.0.1:3700", fetch });
  await client.report();
  await client.report({
    includeNetworkIds: true,
    description: "  stops after an hour ",
  });
  expect(await client.crashNotice()).toBeNull();
  const calls = fetch.mock.calls as unknown as [string, RequestInit][];
  expect(calls.map(([url]) => url)).toEqual([
    "http://127.0.0.1:3700/report",
    "http://127.0.0.1:3700/report",
    "http://127.0.0.1:3700/report/crash",
  ]);
  for (const [, init] of calls) {
    expect(init.method).toBe("POST");
    const headers = new Headers(init.headers);
    expect(headers.get(REPORT_HEADER)).toBe("1");
    expect(headers.get("content-type")).toBe("application/json");
  }
  expect(JSON.parse(String(calls[0][1].body))).toEqual({
    include_network_ids: false,
  });
  expect(JSON.parse(String(calls[1][1].body))).toEqual({
    include_network_ids: true,
    description: "stops after an hour",
  });
});
