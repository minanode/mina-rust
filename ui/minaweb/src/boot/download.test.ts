import { expect, test } from "vitest";

import { BootError, downloadWasm, sha256Hex } from "./download";

const bytes = new TextEncoder().encode("\0asm not really a module");

test("the bytes, with progress, once their SHA-256 matches", async () => {
  const sha256 = await sha256Hex(bytes.buffer);
  const seen: number[] = [];
  const got = await downloadWasm(
    { url: "/x.wasm", sha256, bytes: bytes.length },
    p => seen.push(p.received),
    async () => new Response(bytes),
  );
  expect(new Uint8Array(got)).toEqual(bytes);
  expect(seen.at(-1)).toBe(bytes.length);
});

test("other bytes fail the integrity check and are not returned", async () => {
  const error = await downloadWasm(
    { url: "/x.wasm", sha256: "0".repeat(64), bytes: null },
    () => {},
    async () => new Response(bytes),
  ).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(BootError);
  expect((error as BootError).failure.kind).toBe("integrity");
  expect((error as BootError).failure.detail).toContain("0".repeat(64));
});

test("a missing file and no answer are told apart", async () => {
  const missing = await downloadWasm(
    { url: "/x.wasm", sha256: "0".repeat(64), bytes: null },
    () => {},
    async () => new Response("no", { status: 404 }),
  ).catch((e: BootError) => e.failure);
  expect(missing).toMatchObject({
    kind: "files",
    detail: "/x.wasm answered HTTP 404.",
  });
  const offline = await downloadWasm(
    { url: "/x.wasm", sha256: "0".repeat(64), bytes: null },
    () => {},
    async () => Promise.reject(new TypeError("Failed to fetch")),
  ).catch((e: BootError) => e.failure);
  expect(offline).toMatchObject({ kind: "download" });
});
