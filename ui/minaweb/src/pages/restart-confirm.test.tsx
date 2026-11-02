// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { HandoffError } from "../roles/handoff";
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
// Whether this browser has IndexedDB and WebCrypto.
let supported = true;

vi.mock("../roles/hooks", () => ({ useKeys: () => keys }));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  handoffSupported: () => supported,
}));

const { RestartConfirm } = await import("./restart-confirm");
const worker = { kind: "worker", publicKey: PUBLIC, fee: "1" } as const;

afterEach(() => {
  cleanup();
  supported = true;
});

type Restart = (role: unknown, key: unknown, choice: unknown) => Promise<void>;

function renderConfirm(restart = vi.fn<Restart>(async () => {})) {
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

test("a held key: no passphrase, nothing to choose", async () => {
  keys = { key: held, saved: null };
  const restart = renderConfirm();
  expect(document.querySelector("[data-key-at-risk]")).toBeNull();
  expect(document.querySelector("input")).toBeNull();
  expect(confirm().disabled).toBe(false);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledWith(worker, held, null);
});

test("the browser can't carry it: the passphrase choice, no restart", async () => {
  keys = { key: held, saved: null };
  const restart = vi.fn<Restart>(async (_r, _k, choice) => {
    if (choice === null) throw new HandoffError("This browser couldn't.");
  });
  renderConfirm(restart);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledOnce();
  expect(screen.getByRole("alert").textContent).toMatch(/couldn't/);
  expect(document.querySelector("[data-key-at-risk]")).not.toBeNull();
  expect(confirm().disabled).toBe(true);
  fireEvent.change(document.getElementById("restart-passphrase")!, {
    target: { value: "correct horse battery" },
  });
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenLastCalledWith(worker, held, {
    kind: "carry",
    passphrase: "correct horse battery",
  });
});

test("no IndexedDB: an unsaved key waits to be carried or let go", async () => {
  supported = false;
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
  expect(restart).toHaveBeenCalledWith(worker, held, {
    kind: "carry",
    passphrase: "correct horse battery",
  });
});

test("no IndexedDB: letting it go is a choice the visitor makes", async () => {
  supported = false;
  keys = { key: held, saved: null };
  const restart = renderConfirm();
  fireEvent.click(document.querySelector("[data-action=drop-key]")!);
  expect(confirm().disabled).toBe(false);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledWith(worker, held, { kind: "drop" });
});

test("a refused restart says why, and stays", async () => {
  supported = false;
  keys = { key: held, saved: null };
  const restart = vi.fn<Restart>(async () => {
    throw new Error("This browser's storage is unavailable.");
  });
  renderConfirm(restart);
  fireEvent.click(document.querySelector("[data-action=drop-key]")!);
  await act(async () => fireEvent.click(confirm()));
  expect(screen.getByRole("alert").textContent).toMatch(/storage/);
  expect(confirm().disabled).toBe(false);
});

test("no IndexedDB, a key saved in this browser: nothing to choose", async () => {
  supported = false;
  keys = {
    key: held,
    saved: { publicKey: PUBLIC } as EncryptedKey,
  };
  const restart = renderConfirm();
  expect(document.querySelector("[data-key-at-risk]")).toBeNull();
  expect(confirm().disabled).toBe(false);
  await act(async () => fireEvent.click(confirm()));
  expect(restart).toHaveBeenCalledWith(worker, held, { kind: "drop" });
});
