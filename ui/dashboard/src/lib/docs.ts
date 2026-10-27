/**
 * The node's docs: the Docs pages of minanode.org, built from
 * `ui/minanode-site/app/docs/`. The base is the site's `metadataBase` in
 * `ui/minanode-site/app/layout.tsx`.
 */
export const DOCS_SITE = "https://minanode.org";

/** The Docs home page, for the navigation's Docs link. */
export const DOCS_HOME = `${DOCS_SITE}/docs/`;

/** Docs pages by their path under `/docs/` (with the static export's trailing
 * slash), and a section's id after `#`. `bugReports` is for the Report a
 * problem page; `overview`, `producer` and `worker` for the pages of those
 * names. */
export const DOCS_PAGES = {
  graphql: "run-a-node/#graphql",
  http: "run-a-node/#http-api",
  bugReports: "bug-reports/",
  overview: "dashboard/",
  producer: "block-producer/",
  worker: "snark-worker/",
} as const;

export function docsUrl(page: keyof typeof DOCS_PAGES): string {
  return `${DOCS_SITE}/docs/${DOCS_PAGES[page]}`;
}

/** A query anyone can try: the sync phase and the best tip's height. */
export const EXAMPLE_QUERY =
  "{ syncStatus bestChain(maxLength: 1) { stateHash protocolState { consensusState { blockHeight } } } }";

/** The curl command that sends `query` to `endpoint`. */
export function curlCommand(endpoint: string, query: string): string {
  return [
    `curl -s ${endpoint} \\`,
    "  -H 'content-type: application/json' \\",
    `  -d '${JSON.stringify({ query })}'`,
  ].join("\n");
}
