// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { readSharing, SHARING_KEY } from "../boot/sharing";
import { HandoffError } from "../roles/handoff";
import type { EncryptedKey, HeldKey } from "../roles/keys";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const held: HeldKey = {
  publicKey: PUBLIC,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "generated",
};
const keys: { key: HeldKey | null; saved: EncryptedKey | null } = {
  key: held,
  saved: null,
};

vi.mock("../roles/hooks", () => ({ useKeys: () => keys }));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  handoffSupported: () => true,
}));

const { SharingCard } = await import("./sharing-card");
const producer = { kind: "producer", publicKey: PUBLIC } as const;

afterEach(() => {
  cleanup();
  localStorage.clear();
});

type Restart = (role: unknown, key: unknown, choice: unknown) => Promise<void>;

function renderCard(on: boolean, browsers = 0, restart = vi.fn<Restart>()) {
  render(
    <SharingCard
      on={on}
      browsers={browsers}
      role={producer}
      restart={restart}
    />,
  );
  return restart;
}

const toggle = () => screen.getByRole("switch") as HTMLButtonElement;
const card = () => document.querySelector("[data-sharing]") as HTMLElement;

test("off by default: a switch that's off, and nothing asked yet", () => {
  renderCard(false);
  expect(card().dataset.sharing).toBe("off");
  expect(toggle().getAttribute("aria-checked")).toBe("false");
  expect(toggle().getAttribute("aria-labelledby")).toBeTruthy();
  expect(screen.getByText("Sharing with other browsers")).toBeTruthy();
  expect(screen.getByText("Off")).toBeTruthy();
  expect(document.querySelector("[data-restart-confirm]")).toBeNull();
});

test("on: the browsers connected, of the cap", () => {
  renderCard(true, 2);
  expect(card().dataset.sharing).toBe("on");
  expect(toggle().getAttribute("aria-checked")).toBe("true");
  expect(screen.getByText("On")).toBeTruthy();
  expect(document.querySelector("[data-sharing-detail]")?.textContent).toBe(
    "2 of 4 browsers connected",
  );
});

test("turning it on says what it gives away before anything changes", async () => {
  const restart = renderCard(false);
  fireEvent.click(toggle());
  const confirm = document.querySelector("[data-restart-confirm]")!;
  const text = confirm.textContent ?? "";
  expect(text).toMatch(/anyone running a modified page, get your public IP/);
  expect(text).toMatch(/IPv4, and IPv6 if you have it/);
  expect(text).toMatch(/up to about 130 MiB for each tab you help sync/);
  expect(text).toMatch(/tens of MiB an hour/);
  expect(text).toMatch(/turn it off here at any time/);
  // The Peers page links the docs once; the confirm doesn't repeat it.
  expect(confirm.querySelector("a")).toBeNull();
  expect(toggle().disabled).toBe(true);
  // Nothing has changed yet.
  expect(restart).not.toHaveBeenCalled();
  expect(readSharing()).toBe(false);

  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Turn sharing on" })),
  );
  // The role and the held key cross the restart.
  expect(restart).toHaveBeenCalledWith(producer, held, null);
  expect(localStorage.getItem(SHARING_KEY)).toBe("on");
});

test("cancel changes nothing", () => {
  const restart = renderCard(false);
  fireEvent.click(toggle());
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(document.querySelector("[data-restart-confirm]")).toBeNull();
  expect(restart).not.toHaveBeenCalled();
  expect(readSharing()).toBe(false);
});

test("a restart that doesn't happen leaves sharing off", async () => {
  const restart = vi.fn<Restart>(async () => {
    throw new HandoffError("This browser couldn't.");
  });
  renderCard(false, 0, restart);
  fireEvent.click(toggle());
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Turn sharing on" })),
  );
  expect(restart).toHaveBeenCalledOnce();
  expect(readSharing()).toBe(false);
  expect(screen.getByRole("alert").textContent).toMatch(/couldn't/);
});

test("turning it off is one confirm, and keeps the role and key", async () => {
  localStorage.setItem(SHARING_KEY, "on");
  const restart = renderCard(true, 3);
  fireEvent.click(toggle());
  expect(document.querySelector("[data-restart-confirm]")?.textContent).toMatch(
    /connected to this tab are dropped/,
  );
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "Turn sharing off" })),
  );
  expect(restart).toHaveBeenCalledWith(producer, held, null);
  expect(readSharing()).toBe(false);
});
