import { expect, test } from "vitest";

import { matchRoute, ROUTE_PATTERNS } from "./router";

test("every page has its path", () => {
  expect(matchRoute("/")).toEqual({ page: "overview" });
  expect(matchRoute("/snark-worker")).toEqual({ page: "worker" });
  expect(matchRoute("/snark-worker/")).toEqual({ page: "worker" });
  expect(matchRoute("/block-producer")).toEqual({ page: "producer" });
  expect(matchRoute("/blocks")).toEqual({ page: "blocks" });
  expect(matchRoute("/blocks/578883")).toEqual({ page: "block", id: "578883" });
  expect(matchRoute("/account")).toEqual({ page: "account", publicKey: null });
  expect(matchRoute("/account/")).toEqual({ page: "account", publicKey: null });
  expect(matchRoute("/account/B62qp3")).toEqual({
    page: "account",
    publicKey: "B62qp3",
  });
  expect(matchRoute("/mempool")).toEqual({ page: "mempool" });
  expect(matchRoute("/peers")).toEqual({ page: "peers" });
  expect(matchRoute("/graphql-endpoint")).toEqual({ page: "graphql" });
  expect(matchRoute("/node-status")).toEqual({ page: "status" });
  expect(matchRoute("/report-a-problem")).toEqual({ page: "report" });
});

test("API paths and deeper paths are not pages", () => {
  for (const path of [
    "/status",
    "/accounts",
    "/graphql",
    "/graphiql",
    "/state/peers",
    "/snarker/control",
    "/report",
    "/report/crash",
    "/blocks/1/2",
    "/account//x",
    "//",
    "/peers//",
    "/assets/index.js",
  ]) {
    expect(matchRoute(path).page, path).toBe("not-found");
  }
});

test("routes.json names no API route", () => {
  // The node's routes (crates/node/native/src/http_server): a page path
  // must not equal one, or start with one's first segment.
  const api = [
    "status",
    "build_env",
    "healthz",
    "readyz",
    "make_heartbeat",
    "state",
    "stats",
    "scan-state",
    "snark-pool",
    "snarker",
    "transaction-pool",
    "accounts",
    "send-payment",
    "best-chain-user-commands",
    "discovery",
    "ledger",
    "hardfork",
    "archive",
    "api-docs",
    "graphql",
    "graphiql",
    "mina",
    "metrics",
    "assets",
    "report",
  ];
  for (const pattern of ROUTE_PATTERNS) {
    const first = pattern.split("/")[1];
    expect(api, pattern).not.toContain(first);
  }
});
