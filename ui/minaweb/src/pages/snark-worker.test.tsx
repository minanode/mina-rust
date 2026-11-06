// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { readFileSync } from "node:fs";

import { afterEach, beforeEach, expect, test, vi } from "vitest";

import type { EncryptedKey, HeldKey, KeyGlue } from "../roles/keys";
import type { Account } from "../roles/node";

// Shapes only: not real keys.
const FRESH = `B62q${"1".repeat(51)}`;
const EXISTING = `B62q${"2".repeat(51)}`;
// The WASM's check, with a checksum: this one fails it.
const BAD_CHECKSUM = `B62q${"3".repeat(51)}`;
const held: HeldKey = {
  publicKey: FRESH,
  privateKey: `EK${"4".repeat(50)}`,
  origin: "generated",
};
const glue = {
  account_public_key_valid: (k: string) => k === FRESH || k === EXISTING,
} as KeyGlue;

const state = vi.hoisted(() => ({
  keys: { key: null, saved: null } as {
    key: HeldKey | null;
    saved: EncryptedKey | null;
  },
  restart: vi.fn(),
  role: { kind: "viewer" } as Record<string, unknown>,
  // Per public key: an account, null for none, "error" for a failed read,
  // "loading" for no answer yet. Missing keys have no account.
  accounts: {} as Record<
    string,
    Account | null | "error" | "loading" | { stale: Account }
  >,
  looked: [] as (string | null)[],
}));
vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../app/boot-context", () => ({
  useBoot: () => ({ rpc: null, role: state.role }),
}));
vi.mock("../roles/hooks", () => ({
  useKeys: () => state.keys,
  useKeyGlue: () => glue,
  useRoleNode: () => null,
  useAccount: (pk: string | null) => {
    state.looked.push(pk);
    const a = pk === null ? undefined : (state.accounts[pk] ?? null);
    const base = { refresh: () => {} };
    if (pk === null)
      return {
        ...base,
        data: null,
        error: null,
        updatedAt: null,
        loading: false,
      };
    if (a === "loading")
      return {
        ...base,
        data: null,
        error: null,
        updatedAt: null,
        loading: true,
      };
    if (a === "error")
      return {
        ...base,
        data: null,
        error: new Error("no ledger"),
        updatedAt: null,
        loading: false,
      };
    if (a !== null && a !== undefined && "stale" in a)
      // An earlier answer, then a failed refresh: the hook keeps both.
      return {
        ...base,
        data: a.stale,
        error: new Error("no ledger"),
        updatedAt: 1,
        loading: false,
      };
    return { ...base, data: a, error: null, updatedAt: 1, loading: false };
  },
}));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  handoffSupported: () => true,
}));
vi.mock("../roles/restart", async original => ({
  ...(await original<typeof import("../roles/restart")>()),
  restartWithKey: state.restart,
}));
const { SnarkWorkerPage, WORKER_FEE_DOCS_URL } = await import("./snark-worker");

beforeEach(() => {
  state.restart.mockReset().mockResolvedValue(undefined);
  state.role = { kind: "viewer" };
  state.keys = { key: null, saved: null };
  state.accounts = {
    [EXISTING]: { publicKey: EXISTING, balance: 0n, nonce: 0, delegate: null },
  };
  state.looked = [];
});
afterEach(cleanup);

const field = (id: string) => document.getElementById(id) as HTMLInputElement;
const type = (id: string, value: string) =>
  fireEvent.change(field(id), { target: { value } });
const recipient = () => field("worker-key");
const hint = () => document.getElementById("worker-key-hint")!;
const problem = () => document.querySelector("[data-worker-problem]");
const startButton = () =>
  document.querySelector("[data-action=start-worker]") as HTMLButtonElement;

async function start() {
  fireEvent.click(startButton());
  await act(async () =>
    fireEvent.click(
      screen.getByRole("button", { name: "Restart as SNARK worker" }),
    ),
  );
}

test("a held key: the recipient field shows it, and can be changed", () => {
  state.keys = { key: held, saved: null };
  render(<SnarkWorkerPage />);
  expect(recipient().value).toBe(FRESH);
  expect(recipient().readOnly).toBe(false);
  expect(hint().textContent).toBe("This tab's key. No account on devnet yet.");
  expect(hint().dataset.recipientAccount).toBe("no");
  expect(state.looked).toContain(FRESH);
});

test("a saved key that is not held: the default too", () => {
  state.keys = { key: null, saved: { publicKey: EXISTING } as EncryptedKey };
  render(<SnarkWorkerPage />);
  expect(recipient().value).toBe(EXISTING);
  expect(hint().textContent).toBe("Your saved key. Has an account on devnet.");
});

test("an edited recipient: its account is looked up, and it gets the fees", async () => {
  state.keys = { key: held, saved: null };
  render(<SnarkWorkerPage />);
  type("worker-key", ` ${EXISTING} `);
  expect(state.looked.at(-1)).toBe(EXISTING);
  expect(hint().textContent).toBe("Has an account on devnet.");
  type("worker-fee", "0.004");
  expect(startButton().disabled).toBe(false);
  await start();
  expect(state.restart).toHaveBeenCalledWith(
    { kind: "worker", publicKey: EXISTING, fee: "0.004" },
    held,
    null,
  );
});

test("a recipient with no account under 1 MINA: blocked, one line, one link", () => {
  state.keys = { key: held, saved: null };
  render(<SnarkWorkerPage />);
  type("worker-fee", "0.004");
  expect(startButton().disabled).toBe(true);
  expect(problem()!.textContent).toBe(
    "This recipient has no account. The fee must be at least 1 MINA. SNARK worker docs (opens in a new tab)",
  );
  expect(startButton().getAttribute("aria-describedby")).toBe(problem()!.id);
  const links = problem()!.querySelectorAll("a");
  expect(links).toHaveLength(1);
  expect(links[0].getAttribute("href")).toBe(WORKER_FEE_DOCS_URL);
  // At the account creation fee, it can start.
  type("worker-fee", "1");
  expect(problem()).toBeNull();
  expect(startButton().disabled).toBe(false);
});

test("no account and the fee untouched: it starts at 1 MINA", () => {
  state.keys = { key: held, saved: null };
  render(<SnarkWorkerPage />);
  expect(field("worker-fee").value).toBe("1");
  expect(startButton().disabled).toBe(false);
});

test("a recipient with an account at 0.004: it can start", async () => {
  render(<SnarkWorkerPage />);
  expect(recipient().value).toBe("");
  type("worker-key", EXISTING);
  type("worker-fee", "0.004");
  expect(problem()).toBeNull();
  expect(startButton().disabled).toBe(false);
  await start();
  expect(state.restart).toHaveBeenCalledWith(
    { kind: "worker", publicKey: EXISTING, fee: "0.004" },
    null,
    null,
  );
});

test("a recipient that isn't a public key: no restart", async () => {
  state.keys = { key: held, saved: null };
  render(<SnarkWorkerPage />);
  for (const bad of ["B62", `EK${"3".repeat(50)}`, BAD_CHECKSUM]) {
    type("worker-key", bad);
    expect(startButton().disabled).toBe(true);
    expect(recipient().getAttribute("aria-invalid")).toBe("true");
    expect(hint().textContent).toBe("Not a public key (B62…).");
  }
  type("worker-key", "");
  expect(startButton().disabled).toBe(true);
  expect(problem()!.textContent).toBe("Enter the public key the fees go to.");
  // Valid at Start, then changed while the confirmation is open.
  type("worker-key", EXISTING);
  type("worker-fee", "0.004");
  fireEvent.click(startButton());
  type("worker-key", BAD_CHECKSUM);
  expect(
    screen.queryByRole("button", { name: "Restart as SNARK worker" }),
  ).toBeNull();
  expect(state.restart).not.toHaveBeenCalled();
});

test("an account the node can't read yet: blocked under 1 MINA", () => {
  state.accounts[EXISTING] = "error";
  render(<SnarkWorkerPage />);
  type("worker-key", EXISTING);
  type("worker-fee", "0.004");
  expect(startButton().disabled).toBe(true);
  expect(problem()!.textContent).toContain(
    "The recipient's account can't be read yet.",
  );
  expect(hint().dataset.recipientAccount).toBeUndefined();
});

test("a failed refresh after an answer: blocked under 1 MINA", () => {
  state.accounts[EXISTING] = {
    stale: { publicKey: EXISTING, balance: 0n, nonce: 0, delegate: null },
  };
  render(<SnarkWorkerPage />);
  type("worker-key", EXISTING);
  type("worker-fee", "0.004");
  expect(startButton().disabled).toBe(true);
  expect(problem()!.textContent).toContain(
    "The recipient's account can't be read yet.",
  );
  expect(hint().textContent).toBe("Its account can't be read yet.");
});

test("while the account is looked up: no start under 1 MINA, no link", () => {
  state.accounts[EXISTING] = "loading";
  render(<SnarkWorkerPage />);
  type("worker-key", EXISTING);
  type("worker-fee", "0.004");
  expect(startButton().disabled).toBe(true);
  expect(problem()!.textContent).toBe("Looking up the recipient's account…");
  expect(problem()!.querySelector("a")).toBeNull();
});

test("the docs link points at a section of the SNARK worker page", () => {
  const [, path, anchor] =
    /^https:\/\/minanode\.org(\/docs\/[a-z-]+\/)#([a-z-]+)$/.exec(
      WORKER_FEE_DOCS_URL,
    )!;
  const page = readFileSync(
    `${import.meta.dirname}/../../../minanode-site/app${path}page.tsx`,
    "utf8",
  );
  expect(page).toContain(`id="${anchor}"`);
});
