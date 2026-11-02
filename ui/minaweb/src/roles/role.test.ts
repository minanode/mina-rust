import { describe, expect, test } from "vitest";

import { parseRole, runOptions, runRoleArgs, VIEWER } from "./role";

const PUBLIC = `B62q${"1".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;

describe("the tab's role", () => {
  test("is read on devnet only", () => {
    const worker = JSON.stringify({
      kind: "worker",
      publicKey: PUBLIC,
      fee: "1",
    });
    expect(parseRole(worker, "devnet")).toEqual({
      kind: "worker",
      publicKey: PUBLIC,
      fee: "1",
    });
    expect(parseRole(worker, "mainnet")).toEqual(VIEWER);
  });

  test("anything else is no role", () => {
    for (const text of [
      null,
      "",
      "{",
      JSON.stringify({ kind: "worker", publicKey: PUBLIC, fee: "0" }),
      JSON.stringify({ kind: "worker", publicKey: PUBLIC, fee: "1e3" }),
      JSON.stringify({ kind: "worker", publicKey: "B62", fee: "1" }),
      JSON.stringify({ kind: "producer", publicKey: PRIVATE }),
      JSON.stringify({ kind: "both", publicKey: PUBLIC }),
    ]) {
      expect(parseRole(text, "devnet")).toEqual(VIEWER);
    }
  });

  test("never holds a private key", () => {
    const stored = JSON.stringify({
      kind: "producer",
      publicKey: PUBLIC,
      privateKey: PRIVATE,
    });
    expect(JSON.stringify(parseRole(stored, "devnet"))).not.toContain(PRIVATE);
  });
});

describe("run's arguments", () => {
  test("a worker passes its public key and fee, no producer key", () => {
    expect(
      runRoleArgs({ kind: "worker", publicKey: PUBLIC, fee: "1" }, null),
    ).toEqual({ blockProducer: null, snarker: PUBLIC, snarkerFee: "1" });
  });

  test("a producer passes its key only if it matches", () => {
    const role = { kind: "producer", publicKey: PUBLIC } as const;
    expect(
      runRoleArgs(role, { publicKey: PUBLIC, privateKey: PRIVATE }),
    ).toEqual({ blockProducer: PRIVATE, snarker: null, snarkerFee: null });
    expect(() =>
      runRoleArgs(role, {
        publicKey: `B62q${"2".repeat(51)}`,
        privateKey: PRIVATE,
      }),
    ).toThrow();
    expect(() => runRoleArgs(role, null)).toThrow();
  });

  test("no role turns on peer discovery (PR #173)", () => {
    for (const options of [
      runOptions(true, null),
      runOptions(false, [{ urls: "stun:stun.example.org" }]),
      runOptions(true, null, true),
    ]) {
      expect(options).not.toHaveProperty("peerDiscovery");
    }
    expect(runOptions(true, null)).toEqual({ frontierStore: true });
    expect(runOptions(true, null, true)).toEqual({
      frontierStore: true,
      frontierStoreReset: true,
    });
  });
});
