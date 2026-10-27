/**
 * The node's bug report: `POST /report` (what `mina report` collects, with
 * the same code and redaction, and its pre-filled GitHub issue link) and
 * `POST /report/crash` (whether the node crashed lately). The node answers
 * only its operator's requests: they carry [REPORT_HEADER].
 */

import { bool, num, obj, str } from "./json";

/** The header a report request must carry. */
export const REPORT_HEADER = "x-mina-report";

export interface ReportOptions {
  /** Keep IP addresses and peer ids in the report (left out by default). */
  includeNetworkIds?: boolean;
  /** What happened, in the reporter's words. */
  description?: string;
}

/** A collected, redacted report. */
export interface Report {
  /** When it was collected, ms since the epoch (null if unreadable). */
  generatedMs: number | null;
  generatedAt: string;
  title: string;
  fingerprint: string;
  /** The report, as the issue's body. */
  markdown: string;
  includeNetworkIds: boolean;
  issue: {
    repo: string;
    url: string;
    /** False: the report is too long for a link, which then carries its
     * summary and asks for the downloaded file. */
    complete: boolean;
  };
  /** A name for the downloaded report: `report-<time>.md`. */
  fileName: string;
}

export function parseReport(raw: unknown): Report {
  const r = obj(raw) ?? {};
  const issue = obj(r.issue) ?? {};
  const generatedAt = str(r.generated_at) ?? "";
  const ms = Date.parse(generatedAt);
  return {
    generatedMs: Number.isFinite(ms) ? ms : null,
    generatedAt,
    title: str(r.title) ?? "Report",
    fingerprint: str(r.fingerprint) ?? "",
    markdown: str(r.markdown) ?? "",
    includeNetworkIds: bool(r.include_network_ids) ?? false,
    issue: {
      repo: str(issue.repo) ?? "",
      url: str(issue.url) ?? "",
      complete: bool(issue.complete) ?? true,
    },
    fileName: str(r.file_name) ?? "report.md",
  };
}

/** The report as a downloaded file holds it: its title, then its body, as
 * `mina report --output` writes it. */
export function reportDocument(report: Report): string {
  return `# ${report.title}\n\n${report.markdown}`;
}

/** That the node crashed: when, and the panic's first line. */
export interface CrashNotice {
  crashedMs: number;
  /** Short and redacted. */
  message: string;
  /** In this run; otherwise in the run just before its current start. */
  thisRun: boolean;
  startedMs: number | null;
}

export function parseCrashNotice(raw: unknown): CrashNotice | null {
  const c = obj(obj(raw)?.crash);
  const crashedMs = num(c?.crashed_ms);
  if (!c || crashedMs === null) return null;
  return {
    crashedMs,
    message: str(c.message) ?? "",
    thisRun: bool(c.this_run) ?? false,
    startedMs: num(c.started_ms),
  };
}
