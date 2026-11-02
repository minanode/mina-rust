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

import { joinIceServers, renewEachIceServers, type IceServer } from "./ice";

const relayIssued = (host: string, seconds: number) => [
  { urls: [`stun:${host}:3478`] },
  {
    urls: [`turn:${host}:3478?transport=udp`],
    username: `${Math.floor(Date.now() / 1000) + seconds}:fixture`,
    credential: "fixture",
  },
];

test("joined lists keep every relay's servers but no expired TURN", () => {
  const now = 2_000_000_000_000;
  const live = [
    {
      urls: ["turn:a.example.org"],
      username: `${now / 1000 + 60}:x`,
      credential: "c",
    },
  ];
  const expired = [
    { urls: ["stun:b.example.org"] },
    {
      urls: ["turn:b.example.org", "stun:b.example.org:3479"],
      username: `${now / 1000}:x`,
      credential: "c",
    },
  ];
  expect(joinIceServers([live, null, expired], now)).toEqual([
    ...live,
    { urls: ["stun:b.example.org"] },
    { ...expired[1], urls: ["stun:b.example.org:3479"] },
  ]);
  // Static credentials have no expiry to drop them by.
  const fixed = [
    { urls: ["turn:c.example.org"], username: "u", credential: "c" },
  ];
  expect(joinIceServers([fixed], now)).toEqual(fixed);
});

test("each relay's issuer renews on its own; one down leaves the other's renewals working", async () => {
  clock();
  const applied: unknown[][] = [];
  let downB = false;
  const load = vi.fn(
    async (url: string | null): Promise<IceServer[] | null> => {
      if (url?.includes("b.example.org")) {
        return downB ? null : relayIssued("b.example.org", 20);
      }
      return relayIssued("a.example.org", 10);
    },
  );
  const urls = [
    "https://a.example.org/ice-servers.json",
    "https://b.example.org/ice-servers.json",
  ];
  const stop = renewEachIceServers(
    urls,
    "https://minaweb.example",
    [relayIssued("a.example.org", 10), null],
    servers => applied.push(servers),
    load,
  );
  // B failed at start: it retries after a second and joins A's list.
  await vi.advanceTimersByTimeAsync(1000);
  expect(applied).toHaveLength(1);
  expect(JSON.stringify(applied[0])).toContain("turn:a.example.org");
  expect(JSON.stringify(applied[0])).toContain("turn:b.example.org");
  // B's issuer goes down; A keeps renewing, and once B's credential has
  // expired the node gets A's TURN alone, never an expired one.
  downB = true;
  await vi.advanceTimersByTimeAsync(30_000);
  const last = applied.at(-1)!;
  expect(JSON.stringify(last)).toContain("turn:a.example.org");
  expect(JSON.stringify(last)).not.toContain("turn:b.example.org");
  expect(JSON.stringify(last)).toContain("stun:b.example.org");
  const count = applied.length;
  stop();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(applied).toHaveLength(count);
});

// One misbehaving issuer must never block another's renewals: the node's
// transport (web.js, the module the WASM loads) refuses a whole update that
// holds one bad TURN credential, so these run every update through it.
import { admitIceLists } from "./ice";
// @ts-expect-error web.js, the WASM's own JavaScript, has no type declarations.
import * as transport from "../../../../crates/p2p/src/service_impl/webrtc/web.js";

/** web.js probes each update with an RTCPeerConnection; Node has none. */
class ProbeConnection {
  constructor(private readonly config: RTCConfiguration) {}
  getConfiguration() {
    return this.config;
  }
  close() {}
}

function poisonedStart(bad: () => IceServer[]) {
  clock();
  vi.stubGlobal("RTCPeerConnection", ProbeConnection);
  let badCalls = 0;
  let fixedB = false;
  const load = vi.fn(
    async (url: string | null): Promise<IceServer[] | null> => {
      if (url?.includes("a.example.org")) {
        return relayIssued("a.example.org", 10);
      }
      badCalls++;
      return fixedB ? relayIssued("b.example.org", 20) : bad();
    },
  );
  const installed: IceServer[][] = [];
  const rejected: unknown[] = [];
  const apply = (servers: IceServer[]) => {
    try {
      (
        transport as { updateWebrtcIceServers(s: IceServer[]): void }
      ).updateWebrtcIceServers(servers);
    } catch (error) {
      rejected.push(error);
      throw error;
    }
    installed.push(servers);
  };
  const urls = [
    "https://a.example.org/ice-servers.json",
    "https://b.example.org/ice-servers.json",
  ];
  const stop = renewEachIceServers(
    urls,
    "https://minaweb.example",
    [relayIssued("a.example.org", 10), bad()],
    apply,
    load,
  );
  return {
    installed,
    rejected,
    stop,
    badCalls: () => badCalls,
    fixB: () => {
      fixedB = true;
    },
  };
}

const turnHosts = (servers: IceServer[]) =>
  servers.flatMap(s => s.urls.filter(u => u.startsWith("turn:")));

test.each([
  ["a year ahead", () => relayIssued("b.example.org", 365 * 86400)],
  [
    "a static username",
    () => [
      {
        urls: ["turn:b.example.org:3478"],
        username: "webnode",
        credential: "fixture",
      },
    ],
  ],
])(
  "a bad first list from one issuer (%s) can't block another's renewals",
  async (_, bad) => {
    const run = poisonedStart(bad);
    // Past A's first 10-second credential: A renewed and was installed
    // without B's bad TURN; B was retried on its own backoff, each bad
    // answer refused.
    await vi.advanceTimersByTimeAsync(15_000);
    expect(run.rejected).toHaveLength(0);
    expect(run.installed.length).toBeGreaterThanOrEqual(1);
    for (const servers of run.installed) {
      expect(turnHosts(servers)).toEqual([
        "turn:a.example.org:3478?transport=udp",
      ]);
    }
    expect(run.badCalls()).toBeGreaterThanOrEqual(3);
    // Once B answers properly, it joins.
    run.fixB();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(run.rejected).toHaveLength(0);
    expect(turnHosts(run.installed.at(-1)!)).toEqual([
      "turn:a.example.org:3478?transport=udp",
      "turn:b.example.org:3478?transport=udp",
    ]);
    run.stop();
  },
);

test("a bad renewal keeps the issuer's last good list and the other's renewals", async () => {
  clock();
  vi.stubGlobal("RTCPeerConnection", ProbeConnection);
  let badB = false;
  const load = async (url: string | null): Promise<IceServer[] | null> =>
    url?.includes("a.example.org")
      ? relayIssued("a.example.org", 10)
      : badB
        ? relayIssued("b.example.org", 365 * 86400)
        : relayIssued("b.example.org", 30);
  const installed: IceServer[][] = [];
  const stop = renewEachIceServers(
    [
      "https://a.example.org/ice-servers.json",
      "https://b.example.org/ice-servers.json",
    ],
    "https://minaweb.example",
    [relayIssued("a.example.org", 10), relayIssued("b.example.org", 30)],
    servers => {
      (
        transport as { updateWebrtcIceServers(s: IceServer[]): void }
      ).updateWebrtcIceServers(servers);
      installed.push(servers);
    },
    load,
  );
  badB = true;
  await vi.advanceTimersByTimeAsync(25_000);
  // A renewed every 8 s, each time with B's last good list (still valid).
  expect(installed.length).toBeGreaterThanOrEqual(3);
  for (const servers of installed) {
    expect(turnHosts(servers)).toEqual([
      "turn:a.example.org:3478?transport=udp",
      "turn:b.example.org:3478?transport=udp",
    ]);
    expect(JSON.stringify(servers)).not.toContain(
      String(Math.floor(Date.now() / 1000) + 365 * 86400 - 25),
    );
  }
  stop();
});

test("one list is taken as it is, so a static file still works", () => {
  const fixed = [
    { urls: ["turn:c.example.org"], username: "u", credential: "c" },
  ];
  expect(admitIceLists([fixed])).toEqual([fixed]);
  expect(admitIceLists([fixed, relayIssued("a.example.org", 60)])).toEqual([
    null,
    relayIssued("a.example.org", 60),
  ]);
  expect(admitIceLists([null, [{ urls: ["stun:a.example.org"] }]])).toEqual([
    null,
    null,
  ]);
});
