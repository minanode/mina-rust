import { expect, test } from "vitest";

import { loadIceServers, parseIceServers } from "./ice";

test("ICE servers the node can use, and why others are dropped", () => {
  expect(
    parseIceServers([
      { urls: "stun:turn.example.org:3478" },
      {
        urls: ["turns:turn.example.org:5349"],
        username: "webnode",
        credential: "x",
        extra: 1,
      },
    ]),
  ).toEqual([
    { urls: ["stun:turn.example.org:3478"] },
    {
      urls: ["turns:turn.example.org:5349"],
      username: "webnode",
      credential: "x",
    },
  ]);
  expect(parseIceServers({})).toBe("not a list");
  expect(parseIceServers([{ urls: "http://x" }])).toMatch(/not a STUN/);
  expect(parseIceServers([{ urls: "turn:x:1" }])).toMatch(/without a username/);
});

test("a list that can't be fetched leaves the built-in servers", async () => {
  expect(await loadIceServers(null, "http://localhost/")).toBeNull();
  expect(
    await loadIceServers(
      "/ice.json",
      "http://localhost/",
      async () => new Response("", { status: 404 }),
    ),
  ).toBeNull();
});

// Simulated short TTLs exercise multiple renewals without a day-long soak.
import { afterEach, vi } from "vitest";
import { renewIceServers } from "./ice";
const issued = (seconds: number) => [
  { urls: ["stun:turn.example.org"] },
  {
    urls: ["turn:turn.example.org"],
    username: `${Math.floor(Date.now() / 1000) + seconds}:fixture`,
    credential: "fixture",
  },
];
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
function clock() {
  vi.useFakeTimers();
  vi.setSystemTime(2_000_000_000_000);
  vi.stubGlobal(
    "document",
    Object.assign(new EventTarget(), { visibilityState: "visible" }),
  );
}

test("short TTL renews before expiry repeatedly and stops cleanly", async () => {
  clock();
  const apply = vi.fn();
  const load = vi.fn(async () => issued(10));
  const stop = renewIceServers(
    "/ice",
    "https://example.org",
    issued(10),
    apply,
    load,
  );
  await vi.advanceTimersByTimeAsync(7999);
  expect(load).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(apply).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(8000);
  expect(apply).toHaveBeenCalledTimes(2);
  stop();
  await vi.advanceTimersByTimeAsync(60000);
  expect(load).toHaveBeenCalledTimes(2);
});

test("issuer failure retains working config and recovers after expiry", async () => {
  clock();
  const apply = vi.fn();
  const load = vi.fn().mockResolvedValue(null);
  const stop = renewIceServers(
    "/ice",
    "https://example.org",
    issued(10),
    apply,
    load,
  );
  await vi.advanceTimersByTimeAsync(9000);
  expect(apply).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(2000);
  expect(apply).not.toHaveBeenCalled();
  load.mockImplementation(async () => issued(10));
  await vi.advanceTimersByTimeAsync(4000);
  expect(apply.mock.lastCall?.[0]).toHaveLength(2);
  stop();
});

test("initial outage retries; suspended tab wakes without concurrent requests", async () => {
  clock();
  let resolve!: (value: ReturnType<typeof issued>) => void;
  const load = vi.fn(
    () =>
      new Promise<ReturnType<typeof issued>>(r => {
        resolve = r;
      }),
  );
  const apply = vi.fn();
  const stop = renewIceServers(
    "/ice",
    "https://example.org",
    null,
    apply,
    load,
  );
  await vi.advanceTimersByTimeAsync(1000);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(load).toHaveBeenCalledTimes(1);
  stop();
  resolve(issued(10));
  await Promise.resolve();
  expect(apply).not.toHaveBeenCalled();
});

test("static credentials do not poll", async () => {
  clock();
  const load = vi.fn();
  const stop = renewIceServers(
    "/ice",
    "https://example.org",
    [{ urls: ["stun:example.org"] }],
    vi.fn(),
    load,
  );
  await vi.advanceTimersByTimeAsync(86400_000);
  expect(load).not.toHaveBeenCalled();
  stop();
});

import cases from "../../../../tools/deploy/minaweb/ice-url-cases.json";
import { validIceUrl } from "./ice";

test.each(cases.valid)("accepts supported ICE URL %s", url => {
  expect(validIceUrl(url)).toBe(true);
});
test.each(cases.invalid)("rejects malformed ICE URL %s", url => {
  expect(validIceUrl(url)).toBe(false);
});

test("bad renewal responses preserve the last good set and retry with backoff", async () => {
  clock();
  const malformed = [
    [],
    [{ urls: ["stun:example.org"] }],
    [{ ...issued(10)[1], urls: ["turn:"] }],
    [{ ...issued(10)[1], username: "" }],
    [{ ...issued(10)[1], credential: "" }],
    issued(90000),
    issued(-1),
  ];
  for (const bad of malformed) {
    clock();
    const initial = issued(600);
    const apply = vi.fn();
    const load = vi
      .fn()
      .mockResolvedValueOnce(bad)
      .mockResolvedValueOnce(bad)
      .mockImplementation(async () => issued(10));
    const stop = renewIceServers(
      "/ice",
      "https://example.org",
      initial,
      apply,
      load,
    );
    await vi.advanceTimersByTimeAsync(
      Math.max(
        1000,
        (Number(initial[1].username!.split(":")[0]) * 1000 - Date.now()) * 0.8,
      ),
    );
    expect(apply).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1000);
    expect(apply).not.toHaveBeenCalled();
    expect(load).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1999);
    expect(load).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(apply).toHaveBeenCalledTimes(1);
    stop();
  }
});

test("transport validation failure retries without treating the response as installed", async () => {
  clock();
  const apply = vi.fn().mockImplementationOnce(() => {
    throw new Error("invalid ICE renewal configuration");
  });
  const load = vi.fn(async () => issued(10));
  const stop = renewIceServers(
    "/ice",
    "https://example.org",
    issued(10),
    apply,
    load,
  );
  await vi.advanceTimersByTimeAsync(8000);
  expect(apply).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(apply).toHaveBeenCalledTimes(2);
  stop();
});
