import { describe, expect, test, vi } from "vitest";

import { createNodeClient, RAW_ROUTES } from "./client";
import { errorMessage, isUnreachable, kindOf, NodeError } from "./errors";
import fixture from "./fixtures/status-devnet.json";

/** A client whose fetch answers `status` with `body`, and records calls. */
function answering(status: number, body: string, baseUrl?: string) {
  const fetch = vi.fn(async () => new Response(body, { status }));
  return { fetch, client: createNodeClient({ baseUrl, fetch }) };
}

const calls = (fetch: ReturnType<typeof vi.fn>) =>
  fetch.mock.calls as unknown as [string, RequestInit][];

test("the node's own words for an error", () => {
  expect(errorMessage('{"error":"worker control is off"}')).toBe(
    "worker control is off",
  );
  expect(errorMessage('"invalid_input"')).toBe("invalid_input");
  expect(errorMessage("plain text")).toBe("plain text");
  expect(errorMessage("  ")).toBeNull();
});

describe("the base URL", () => {
  test("empty: the page's own origin", async () => {
    const { fetch, client } = answering(200, "{}");
    await client.getJson("/status", x => x);
    expect(calls(fetch)[0][0]).toBe("/status");
    expect(client.url("graphql")).toBe("/graphql");
  });
  test("another node, with or without a trailing slash or a path", async () => {
    for (const base of [
      "http://192.0.2.44:3600",
      "http://192.0.2.44:3600/",
    ]) {
      const { fetch, client } = answering(200, "{}", base);
      await client.getJson("/status", x => x);
      expect(calls(fetch)[0][0]).toBe("http://192.0.2.44:3600/status");
    }
    const proxied = createNodeClient({ baseUrl: "https://example.org/node/" });
    expect(proxied.url("/stats/sync?limit=1")).toBe(
      "https://example.org/node/stats/sync?limit=1",
    );
  });
});

describe("what a failed request means", () => {
  test("no answer", async () => {
    const client = createNodeClient({
      fetch: vi.fn(async () =>
        Promise.reject(new TypeError("Failed to fetch")),
      ),
    });
    const error = await client.status().catch((e: unknown) => e);
    expect(error).toMatchObject({ kind: "offline" });
    expect(isUnreachable(error)).toBe(true);
  });
  test("an answer that breaks off after its headers", async () => {
    const broken = new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('{"network":'));
        controller.error(new TypeError("network error"));
      },
    });
    const client = createNodeClient({
      fetch: vi.fn(async () => new Response(broken, { status: 200 })),
    });
    const error = await client.status().catch((e: unknown) => e);
    expect(error).toMatchObject({
      kind: "offline",
      message: "The node stopped answering mid-reply.",
    });
    expect(isUnreachable(error)).toBe(true);
  });
  test("a caller that gives up gets its own abort, not offline", async () => {
    const controller = new AbortController();
    const client = createNodeClient({
      fetch: vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
        controller.abort();
        throw init?.signal?.reason ?? new DOMException("aborted", "AbortError");
      }),
    });
    const error = await client
      .status(controller.signal)
      .catch((e: unknown) => e);
    expect(error).not.toBeInstanceOf(NodeError);
  });
  test("a route the node doesn't serve: an older or public node", async () => {
    const { client } = answering(404, "");
    const error = await client.workerControl().catch((e: unknown) => e);
    expect(error).toMatchObject({
      kind: "absent",
      message: "This node doesn't serve /snarker/control.",
    });
    expect(isUnreachable(error)).toBe(false);
  });
  test("a refusal (a feature turned off) keeps the node's reason and status", async () => {
    const { client } = answering(
      403,
      '{"error":"worker control is off: start the node with --allow-worker-control"}',
    );
    const error = (await client
      .changeWorker({ enabled: false })
      .catch((e: unknown) => e)) as NodeError;
    expect(error).toBeInstanceOf(NodeError);
    expect(error).toMatchObject({ kind: "refused", status: 403 });
    expect(error.message).toMatch(/--allow-worker-control/);
  });
  test("a server error", async () => {
    const { client } = answering(500, '{"error":"response channel dropped"}');
    const error = await client.status().catch((e: unknown) => e);
    expect(error).toMatchObject({
      kind: "failed",
      status: 500,
      message: "response channel dropped",
    });
    expect(isUnreachable(error)).toBe(true);
  });
  test("an answer that isn't JSON", async () => {
    const { client } = answering(200, "<html>");
    await expect(client.status()).rejects.toMatchObject({ kind: "invalid" });
  });
  test("anything else is failed", () => {
    expect(kindOf(new Error("x"))).toBe("failed");
  });
});

test("a worker change sends JSON with the control header", async () => {
  const { fetch, client } = answering(200, "{}");
  await client.changeWorker({ enabled: false });
  const [path, init] = calls(fetch)[0];
  expect(path).toBe("/snarker/control");
  expect(init.method).toBe("POST");
  expect(init.body).toBe('{"enabled":false}');
  expect(init.headers).toMatchObject({
    "Content-Type": "application/json",
    "x-mina-worker-control": "1",
  });
});

test("each endpoint asks its route and parses the answer", async () => {
  const { fetch, client } = answering(200, JSON.stringify(fixture.status));
  const status = await client.status();
  expect(status.bestTip?.height).toBe(578883);
  const paths: string[] = [];
  for (const call of [
    () => client.buildInfo(),
    () => client.syncRestore(),
    () => client.producerStats(),
    () => client.workerControl(),
    () => client.includedWork(),
    () => client.transactionPool(),
    () => client.peers(),
  ]) {
    await call().catch(() => {});
  }
  for (const [path] of calls(fetch)) paths.push(path);
  expect(paths).toEqual([
    "/status",
    "/build_env",
    "/stats/sync?limit=1",
    "/stats/block_producer",
    "/snarker/control",
    "/snarker/included",
    "/transaction-pool",
    "/state/peers",
  ]);
  expect(RAW_ROUTES).toEqual(["/status", "/build_env"]);
  expect(RAW_ROUTES.some(route => route.startsWith("/state"))).toBe(false);
});

describe("GraphQL", () => {
  test("data", async () => {
    const { client } = answering(200, '{"data":{"version":"x"}}');
    await expect(
      client.graphql("{ version }", {}, d => d.version),
    ).resolves.toBe("x");
  });
  test("an error without data is the node's message", async () => {
    const { client } = answering(
      200,
      '{"errors":[{"message":"Block with state hash 3N... not found in transition frontier"}]}',
    );
    await expect(client.graphql("q", {}, d => d)).rejects.toMatchObject({
      kind: "refused",
      message: "Block with state hash 3N... not found in transition frontier",
    });
  });
  test("a field error with null data is the node's message too", async () => {
    const { client } = answering(
      200,
      '{"data":{"block":null},"errors":[{"message":"Could not find block in transition frontier with height 5"}]}',
    );
    await expect(client.graphql("q", {}, d => d)).rejects.toMatchObject({
      message: "Could not find block in transition frontier with height 5",
    });
  });
  test("blocks by height or hash, and a refusal for anything else", async () => {
    const { fetch, client } = answering(200, '{"data":{"block":null}}');
    await expect(client.block("578883")).resolves.toBeNull();
    expect(JSON.parse(calls(fetch)[0][1].body as string).variables).toEqual({
      height: 578883,
    });
    await expect(client.block("nope")).rejects.toMatchObject({
      kind: "refused",
    });
    expect(calls(fetch)).toHaveLength(1);
  });
});
