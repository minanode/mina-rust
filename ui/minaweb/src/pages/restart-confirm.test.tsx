// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import type { EncryptedKey, HeldKey } from "../roles/keys";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const held: HeldKey = {
  publicKey: PUBLIC,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "generated",
};
let keys: { key: HeldKey | null; saved: EncryptedKey | null } = {
  key: held,
  saved: null,
};

vi.mock("../roles/hooks", () => ({ useKeys: () => keys }));

const { RestartConfirm } = await import("./restart-confirm");
const worker = { kind: "worker", publicKey: PUBLIC, fee: "1" } as const;

afterEach(cleanup);

function renderConfirm() {
  const restart = vi.fn(async () => {});
  render(
    <RestartConfirm
      label="Confirm starting the SNARK worker"
      message="The node starts again."
      confirmLabel="Start it"
      role={worker}
      onCancel={() => {}}
      restart={restart}
    />,
  );
  return restart;
}

const confirm = () =>
  screen.getByRole("button", { name: "Start it" }) as HTMLButtonElement;

test("an unsaved key: no restart until it is carried or let go", async () => {
  keys = { key: held, saved: null };
  const restart = renderConfirm();
  expect(document.querySelector("[data-key-at-risk]")).not.toBeNull();
  expect(confirm().disabled).toBe(true);

  // A short passphrase isn't enough.
  fireEvent.change(document.getElementById("restart-passphrase")!, {
    target: { value: "short" },
  });
  expect(confirm().disabled).toBe(true);
  fireEvent.change(document.getElementById("restart-passphrase")!, {
    target: { value: "correct horse battery" },
  });
  expect(confirm().disabled).toBe(false);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledWith(worker, held, null, {
    kind: "carry",
    passphrase: "correct horse battery",
  });
});

test("letting it go is a choice the visitor makes", async () => {
  keys = { key: held, saved: null };
  const restart = renderConfirm();
  fireEvent.click(document.querySelector("[data-action=drop-key]")!);
  expect(confirm().disabled).toBe(false);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledWith(worker, held, null, { kind: "drop" });
});

test("a refused restart says why, and stays", async () => {
  keys = { key: held, saved: null };
  const restart = vi.fn(async () => {
    throw new Error("This browser's storage is unavailable.");
  });
  render(
    <RestartConfirm
      label="Confirm"
      message="m"
      confirmLabel="Start it"
      role={worker}
      onCancel={() => {}}
      restart={restart}
    />,
  );
  fireEvent.click(document.querySelector("[data-action=drop-key]")!);
  await act(async () => fireEvent.click(confirm()));
  expect(screen.getByRole("alert").textContent).toMatch(/storage/);
  expect(confirm().disabled).toBe(false);
});

test("a key saved in this browser: nothing to choose", () => {
  keys = {
    key: held,
    saved: { publicKey: PUBLIC } as EncryptedKey,
  };
  renderConfirm();
  expect(document.querySelector("[data-key-at-risk]")).toBeNull();
  expect(confirm().disabled).toBe(false);
});
