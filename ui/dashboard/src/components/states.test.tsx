// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";

import type { Polled } from "@minanode/node-client/react";

import { Loaded } from "./states";

afterEach(cleanup);

const polled = (
  over: Partial<Polled<string | null>>,
): Polled<string | null> => ({
  data: null,
  error: null,
  updatedAt: null,
  loading: true,
  refresh: () => {},
  ...over,
});

const show = (p: Polled<string | null>) =>
  render(
    <Loaded polled={p} what="the thing">
      {value => <p>{value === null ? "Answered: none" : value}</p>}
    </Loaded>,
  );

test("an answer of null reaches the page, not a placeholder forever", () => {
  show(polled({ loading: false, updatedAt: Date.now() }));
  expect(screen.getByText("Answered: none")).toBeTruthy();
});

test("before the first answer, a placeholder", () => {
  show(polled({}));
  expect(screen.queryByText("Answered: none")).toBeNull();
  expect(screen.getByRole("status").textContent).toBe("Loading the thing…");
});
