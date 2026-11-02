import { expect, test } from "vitest";

import { matchRoute } from "./router";

test("the pages' routes", () => {
  const hash = "3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD";
  const key = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
  expect(matchRoute("/blocks")).toEqual({ page: "blocks" });
  expect(matchRoute(`/blocks/${hash}`)).toEqual({ page: "block", id: hash });
  expect(matchRoute("/blocks/581331/")).toEqual({
    page: "block",
    id: "581331",
  });
  expect(matchRoute("/accounts")).toEqual({
    page: "accounts",
    publicKey: null,
  });
  expect(matchRoute(`/accounts/${key}`)).toEqual({
    page: "accounts",
    publicKey: key,
  });
  expect(matchRoute("/pool")).toEqual({ page: "pool" });
  expect(matchRoute("/blocks/a/b").page).toBe("not-found");
  expect(matchRoute("/accounts/%E0%A4%A").page).toBe("not-found");
});
