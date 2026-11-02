// MINAWEB-CSP: collect Content-Security-Policy violations while a check
// drives the web node page, so that the check can fail on any. Two channels:
// a securitypolicyviolation listener in every document (frames included)
// and Chrome's own console reports, from the page, its frames and its
// workers; and, more complete, the reports the policy sends to serve.py's
// report endpoint when it runs with CSP_REPORTS=FILE, which cover workers
// and the prover's iframe too.
import { existsSync, readFileSync } from "node:fs";

const TAG = "[csp-violation]";
const CONSOLE = /Content.Security.Policy|\[csp-violation\]/i;

/** Watches `page` (a puppeteer Page); calls `record(text)` for each
 * violation seen in a document or a worker. */
export async function watchCsp(page, record) {
  await page.evaluateOnNewDocument(tag => {
    addEventListener("securitypolicyviolation", e => {
      console.error(
        `${tag} ${JSON.stringify({
          directive: e.effectiveDirective,
          blocked: e.blockedURI,
          document: e.documentURI,
          source: e.sourceFile,
          line: e.lineNumber,
          sample: e.sample,
        })}`,
      );
    });
  }, TAG);
  const seen = message => {
    const text = message.text();
    if (CONSOLE.test(text)) record(text);
  };
  page.on("console", seen);
  page.on("workercreated", worker => worker.on("console", seen));
}

/** The reports serve.py wrote to `file` (CSP_REPORTS), one per line. */
export function reportedViolations(file) {
  if (!file || !existsSync(file)) return [];
  return readFileSync(file, "utf8")
    .split("\n")
    .filter(Boolean)
    .map(line => {
      const report = JSON.parse(line)["csp-report"] ?? JSON.parse(line);
      return {
        directive: report["effective-directive"] ?? report["violated-directive"],
        blocked: report["blocked-uri"],
        document: report["document-uri"],
        source: report["source-file"],
        line: report["line-number"],
        sample: report["script-sample"],
      };
    });
}

/** The policy `base` serves its page with, checked against the build's
 * (build-info.json's contentSecurityPolicy, plus serve.py's report-uri when
 * it collects reports). Throws if the page isn't served under it. */
export async function servedPolicy(base) {
  const info = await (await fetch(`${base}/build-info.json`)).json();
  const page = await fetch(`${base}/`);
  const served = page.headers.get("content-security-policy");
  const expected = info.contentSecurityPolicy;
  if (!expected) throw new Error("build-info.json has no contentSecurityPolicy: build from a commit that makes one");
  if (served !== expected && !served?.startsWith(`${expected}; report-uri `)) {
    throw new Error(`the page is served with Content-Security-Policy ${JSON.stringify(served)}, not the build's`);
  }
  for (const [name, value] of [["x-frame-options", "DENY"]]) {
    if (page.headers.get(name) !== value) throw new Error(`the page is served without ${name}: ${value}`);
  }
  return served;
}
