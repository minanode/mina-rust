import { expect, test } from "vitest";

import { lowMemoryWarning, startCost } from "./resources";

test("a device that reports little memory gets a warning, not a refusal", () => {
  expect(lowMemoryWarning("producer", null)).toBeNull();
  expect(lowMemoryWarning("producer", 8)).toBeNull();
  expect(lowMemoryWarning("worker", 4)).toMatch(/You can still start it/);
  expect(lowMemoryWarning("producer", 2)).toMatch(/2 GB/);
});

test("the start screen's line: the WASM and prover files Start downloads, and the tab's peak", () => {
  const wasm = 39_160_000;
  expect(startCost("viewer", wasm)).toBe(
    "Starting it downloads the node (39.2 MB of WebAssembly). It uses about 1.3 GiB of memory.",
  );
  expect(startCost("worker", wasm)).toBe(
    "Starting it as a SNARK worker downloads the node (39.2 MB of WebAssembly) and 49.5 MB of prover files. It uses about 3.1 GiB of memory.",
  );
  expect(startCost("producer", wasm)).toBe(
    "Starting it as a block producer downloads the node (39.2 MB of WebAssembly) and 45.4 MB of prover files. It uses about 4.0 GiB of memory.",
  );
  expect(startCost("viewer", null)).toBe(
    "Starting it downloads the node. It uses about 1.3 GiB of memory.",
  );
});
