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
  const layout = readFileSync(new URL("app/layout.tsx", site), "utf8");
  const base = /metadataBase: new URL\("([^"]+)"\)/.exec(layout)?.[1];
  expect(base?.replace(/\/$/, "")).toBe(DOCS_SITE);
});

test("the curl command sends the query as JSON", () => {
  const command = curlCommand(
    "http://192.0.2.44:3700/graphql",
    EXAMPLE_QUERY,
  );
  expect(command.split("\n")[0]).toBe(
    "curl -s http://192.0.2.44:3700/graphql \\",
  );
  const body = /-d '(.*)'$/.exec(command)?.[1];
  expect(JSON.parse(body!)).toEqual({ query: EXAMPLE_QUERY });
  expect(EXAMPLE_QUERY).not.toContain("'");
});
