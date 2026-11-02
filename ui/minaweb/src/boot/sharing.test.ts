// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";

import { StorageError } from "../roles/keys";
import {
  readSharing,
  SHARING_KEY,
  SHARING_MAX_PEERS,
  writeSharing,
} from "./sharing";

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

test("off by default, and for anything but on", () => {
  expect(readSharing()).toBe(false);
  for (const value of ["", "true", "1", "ON", "off"]) {
    localStorage.setItem(SHARING_KEY, value);
    expect(readSharing()).toBe(false);
  }
});

test("the choice is this browser's, kept across tabs and reloads", () => {
  writeSharing(true);
  expect(localStorage.getItem(SHARING_KEY)).toBe("on");
  expect(readSharing()).toBe(true);
  writeSharing(false);
  expect(localStorage.getItem(SHARING_KEY)).toBeNull();
  expect(readSharing()).toBe(false);
});

test("a choice storage refuses throws, so no restart starts the old way", () => {
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("quota");
  });
  expect(() => writeSharing(true)).toThrow(StorageError);
  vi.restoreAllMocks();
  localStorage.setItem(SHARING_KEY, "on");
  vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {});
  expect(() => writeSharing(false)).toThrow(StorageError);
});

test("the page's cap is the node's", () => {
  // crates/p2p/src/sharing.rs, P2pSharingPeerLimits::default().max_peers.
  expect(SHARING_MAX_PEERS).toBe(4);
});
