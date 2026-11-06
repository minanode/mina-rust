import { existsSync, readFileSync } from "node:fs";

import { expect, test } from "vitest";

import {
  curlCommand,
  DOCS_HOME,
  DOCS_PAGES,
  DOCS_SITE,
  docsUrl,
  EXAMPLE_QUERY,
} from "./docs";

const repo = new URL("../../../../", import.meta.url);

const site = new URL("ui/minanode-site/", repo);

test("the docs links point at pages and sections of minanode.org's Docs", () => {
  for (const page of Object.values(DOCS_PAGES)) {
    const [path, section] = page.split("#");
    expect(path, page).toMatch(/^[a-z-]+\/$/);
    const source = new URL(`app/docs/${path}page.tsx`, site);
    expect(existsSync(source), page).toBe(true);
    if (section) {
      expect(readFileSync(source, "utf8"), page).toContain(`id="${section}"`);
    }
  }
  expect(docsUrl("graphql")).toBe(
    "https://minanode.org/docs/run-a-node/#graphql",
  );
  expect(docsUrl("producer")).toBe("https://minanode.org/docs/block-producer/");
  expect(DOCS_HOME).toBe("https://minanode.org/docs/");
  expect(existsSync(new URL("app/docs/page.tsx", site))).toBe(true);
});

test("the docs site is minanode.org's own address", () => {
  // The site's metadataBase is its origin, minanode.org unless a preview
  // build sets another.
  const layout = readFileSync(new URL("app/layout.tsx", site), "utf8");
  expect(layout).toMatch(/metadataBase: new URL\(siteOrigin\)/);
  const origin = readFileSync(new URL("lib/origin.ts", site), "utf8");
  const base = /\?\?\s*"([^"]+)"/.exec(origin)?.[1];
  expect(base?.replace(/\/$/, "")).toBe(DOCS_SITE);
});

test("the curl command sends the query as JSON", () => {
  const command = curlCommand("http://192.0.2.44:3700/graphql", EXAMPLE_QUERY);
  expect(command.split("\n").slice(0, 2)).toEqual([
    "curl -s \\",
    "  http://192.0.2.44:3700/graphql \\",
  ]);
  const body = /-d '(.*)'$/.exec(command)?.[1];
  expect(JSON.parse(body!)).toEqual({ query: EXAMPLE_QUERY });
  expect(EXAMPLE_QUERY).not.toContain("'");
});
