// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

let statusHeight: number | null = null;
const refresh = vi.fn();

vi.mock("@minanode/node-client/react", () => ({
  useNodeStatus: () => ({
    status: {
      data:
        statusHeight === null ? null : { bestTip: { height: statusHeight } },
      error: null,
      updatedAt: 1,
      loading: false,
      refresh,
    },
  }),
}));

const { statusBehind, useStatusCatchUp, useStatusOnNavigate } =
  await import("./tip");
const { navigate } = await import("../app/router");

beforeEach(() => {
  refresh.mockClear();
  window.history.replaceState(null, "", "/");
});
afterEach(cleanup);

test("the status is behind only a higher block it doesn't hold", () => {
  expect(statusBehind(583745, 583746)).toBe(true);
  expect(statusBehind(583746, 583746)).toBe(false);
  expect(statusBehind(583747, 583746)).toBe(false);
  expect(statusBehind(null, 583746)).toBe(false);
  expect(statusBehind(583745, null)).toBe(false);
});

function CatchUp({ seen }: { seen: number | null }) {
  useStatusCatchUp(seen);
  return null;
}

test("a page holding a newer block asks the status again, once", () => {
  statusHeight = 583745;
  const { rerender } = render(<CatchUp seen={583745} />);
  expect(refresh).not.toHaveBeenCalled();

  rerender(<CatchUp seen={583746} />);
  expect(refresh).toHaveBeenCalledTimes(1);
  // The same heights again: no second request.
  rerender(<CatchUp seen={583746} />);
  expect(refresh).toHaveBeenCalledTimes(1);

  // The status caught up: nothing more to ask.
  statusHeight = 583746;
  rerender(<CatchUp seen={583746} />);
  expect(refresh).toHaveBeenCalledTimes(1);
});

function OnNavigate() {
  useStatusOnNavigate();
  return null;
}

test("a navigation asks the status again, the first page doesn't", () => {
  statusHeight = 583745;
  render(<OnNavigate />);
  expect(refresh).not.toHaveBeenCalled();

  act(() => navigate("/blocks"));
  expect(refresh).toHaveBeenCalledTimes(1);
  act(() => navigate("/blocks/3NLrCXjqmbhEHSqm5epDNxaQ8eSzby6ox1k2NDicZvHa"));
  expect(refresh).toHaveBeenCalledTimes(2);
});
