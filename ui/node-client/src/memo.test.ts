import { expect, test } from "vitest";

import { decodeMemo } from "./memo";

// Memos with known text, their checksums verified independently (Python):
// OCaml's empty memo from mainnet block 552000 (crates/cli wallet tests), a
// memo from the p2p-messages hashing tests, and one in the devnet mempool.
test("decodes memos", () => {
  expect(
    decodeMemo("E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH"),
  ).toBe("");
  expect(
    decodeMemo("E4Yks7aARFemZJqucP5eaARRYRthGdzaFjGfXqQRS3UeidsECRBvR"),
  ).toBe("BorkBorkBork");
  expect(
    decodeMemo("E4YzjHJ6ds2xcGuWvfVGQzp6HXhk5R2c5xrj9apXH5ER941pRxqmS"),
  ).toBe("devnet-tx-generator");
});

test("anything else is not a memo", () => {
  for (const bad of [
    null,
    undefined,
    "",
    "0OIl",
    "E4Y",
    "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
  ]) {
    expect(decodeMemo(bad)).toBeNull();
  }
});
