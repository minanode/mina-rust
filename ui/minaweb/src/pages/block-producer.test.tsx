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

import { createNodeClient } from "@minanode/node-client";
import {
  NodeClientProvider,
  NodeStatusProvider,
} from "@minanode/node-client/react";

import recorded from "../../../node-client/src/fixtures/producer-attempts-devnet-3610.json";
import type { EncryptedKey, HeldKey, KeyGlue } from "../roles/keys";

// Shapes only: not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const held: HeldKey = {
  publicKey: PUBLIC,
  privateKey: `EK${"3".repeat(50)}`,
  origin: "imported",
};
const saved = { publicKey: PUBLIC } as EncryptedKey;
const RECEIVER = `B62q${"2".repeat(51)}`;
// The WASM's check, with a checksum: this one fails it.
const BAD_CHECKSUM = `B62q${"3".repeat(51)}`;
const glue = {
  account_public_key_valid: (k: string) => k === PUBLIC || k === RECEIVER,
} as KeyGlue;

const state = vi.hoisted(() => ({
  keys: { key: null, saved: null } as {
    key: HeldKey | null;
    saved: EncryptedKey | null;
  },
  restart: vi.fn(),
  role: { kind: "viewer" } as Record<string, unknown>,
  glue: null as KeyGlue | null,
}));
vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../app/boot-context", () => ({
  useBoot: () => ({ rpc: null, role: state.role }),
}));
vi.mock("../roles/hooks", () => ({
  useKeys: () => state.keys,
  useKeyGlue: () => state.glue,
  useRoleNode: () => null,
}));
vi.mock("../roles/handoff", async original => ({
  ...(await original<typeof import("../roles/handoff")>()),
  handoffSupported: () => true,
}));
vi.mock("../roles/restart", async original => ({
  ...(await original<typeof import("../roles/restart")>()),
  restartWithKey: state.restart,
}));
const { keyStore } = await import("../roles/keys");
const { BlockProducerPage, PRODUCER_DOCS_URL } =
  await import("./block-producer");
const { ONE_ROLE_DOCS_URL } = await import("./roles-common");
const producer = { kind: "producer", publicKey: PUBLIC };

beforeEach(() => {
  state.restart.mockReset().mockResolvedValue(undefined);
  state.role = { kind: "viewer" };
  state.glue = null;
  keyStore.forget();
});
afterEach(cleanup);

const passwordFields = () =>
  document.querySelectorAll('input[type="password"]').length;

async function startProducing() {
  fireEvent.click(document.querySelector("[data-action=start-producer]")!);
  await act(async () =>
    fireEvent.click(
      screen.getByRole("button", { name: "Restart as block producer" }),
    ),
  );
}

test("a key held in the tab: Start asks for nothing", async () => {
  state.keys = { key: held, saved: null };
  render(<BlockProducerPage />);
  expect(passwordFields()).toBe(0);
  expect(screen.queryByText(/Passphrase for the restart/)).toBeNull();
  await startProducing();
  expect(state.restart).toHaveBeenCalledWith(producer, held, null);
});

test("a key saved here but not held: its passphrase, once", async () => {
  state.keys = { key: null, saved };
  const unlock = vi
    .spyOn(keyStore, "unlockSaved")
    .mockImplementation(async passphrase => {
      if (passphrase !== "the saved one") throw new Error("Wrong passphrase.");
      keyStore.hold(held);
    });
  render(<BlockProducerPage />);
  expect(passwordFields()).toBe(1);
  const start = document.querySelector(
    "[data-action=start-producer]",
  ) as HTMLButtonElement;
  expect(start.disabled).toBe(true);
  fireEvent.change(document.getElementById("producer-saved-passphrase")!, {
    target: { value: "the saved one" },
  });
  await startProducing();
  expect(unlock).toHaveBeenCalledOnce();
  // Then as a held key: carried with no passphrase of its own.
  expect(state.restart).toHaveBeenCalledWith(producer, held, null);
  unlock.mockRestore();
});

test("a wrong saved passphrase: no restart", async () => {
  state.keys = { key: null, saved };
  const unlock = vi
    .spyOn(keyStore, "unlockSaved")
    .mockRejectedValue(new Error("Wrong passphrase."));
  render(<BlockProducerPage />);
  fireEvent.change(document.getElementById("producer-saved-passphrase")!, {
    target: { value: "not it" },
  });
  await startProducing();
  expect(screen.getByText("Wrong passphrase.")).toBeTruthy();
  expect(state.restart).not.toHaveBeenCalled();
  unlock.mockRestore();
});

test("no key: the key screen", () => {
  state.keys = { key: null, saved: null };
  render(<BlockProducerPage />);
  expect(screen.getByText("No key in this tab")).toBeTruthy();
});

test("the browser can't carry the key: the passphrase for the restart", async () => {
  state.keys = { key: held, saved: null };
  const { HandoffError } = await import("../roles/handoff");
  state.restart.mockImplementation(async (_r, _k, choice) => {
    if (choice === null) throw new HandoffError("This browser couldn't.");
  });
  render(<BlockProducerPage />);
  await startProducing();
  expect(screen.getByText(/couldn't/)).toBeTruthy();
  expect(screen.getByText("Passphrase for the restart")).toBeTruthy();
  fireEvent.change(document.getElementById("producer-passphrase")!, {
    target: { value: "correct horse battery" },
  });
  await startProducing();
  expect(state.restart).toHaveBeenLastCalledWith(producer, held, {
    kind: "carry",
    passphrase: "correct horse battery",
  });
});

const receiverFieldOrNull = () =>
  document.getElementById("producer-coinbase-receiver");
const receiverField = () => receiverFieldOrNull() as HTMLInputElement;
const startButton = () =>
  document.querySelector(
    "[data-action=start-producer]",
  ) as HTMLButtonElement | null;

test("an empty coinbase receiver: none passed, the producer key gets it", async () => {
  state.keys = { key: held, saved: null };
  state.glue = glue;
  render(<BlockProducerPage />);
  expect(receiverField().value).toBe("");
  fireEvent.change(receiverField(), { target: { value: "   " } });
  await startProducing();
  expect(state.restart).toHaveBeenCalledWith(producer, held, null);
  expect(state.restart.mock.calls[0][0]).not.toHaveProperty("coinbaseReceiver");
});

test("a coinbase receiver: checked, and kept in the role", async () => {
  state.keys = { key: held, saved: null };
  state.glue = glue;
  render(<BlockProducerPage />);
  fireEvent.change(receiverField(), { target: { value: ` ${RECEIVER} ` } });
  expect(startButton()!.disabled).toBe(false);
  await startProducing();
  expect(state.restart).toHaveBeenCalledWith(
    { ...producer, coinbaseReceiver: RECEIVER },
    held,
    null,
  );
});

test("a receiver that isn't a public key: no restart", async () => {
  state.keys = { key: held, saved: null };
  state.glue = glue;
  render(<BlockProducerPage />);
  for (const bad of ["B62", `EK${"3".repeat(50)}`, BAD_CHECKSUM]) {
    fireEvent.change(receiverField(), { target: { value: bad } });
    expect(startButton()!.disabled).toBe(true);
    expect(receiverField().getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByText("Not a public key (B62…).")).toBeTruthy();
  }
  // Valid at Start, then changed while the confirmation is open.
  fireEvent.change(receiverField(), { target: { value: RECEIVER } });
  fireEvent.click(startButton()!);
  fireEvent.change(receiverField(), { target: { value: BAD_CHECKSUM } });
  await act(async () =>
    fireEvent.click(
      screen.getByRole("button", { name: "Restart as block producer" }),
    ),
  );
  expect(state.restart).not.toHaveBeenCalled();
});

test("no key check in this build: a typed receiver is refused", () => {
  state.keys = { key: held, saved: null };
  render(<BlockProducerPage />);
  fireEvent.change(receiverField(), { target: { value: RECEIVER } });
  expect(startButton()!.disabled).toBe(true);
});

test("running: the Producer card shows the node's coinbase receiver", async () => {
  state.keys = { key: held, saved: null };
  state.role = { ...producer, coinbaseReceiver: RECEIVER };
  const json = (value: unknown, status = 200) =>
    new Response(JSON.stringify(value), { status });
  const fetch = vi.fn(async (url: string | URL | Request) => {
    const path = String(url);
    if (path === "/status")
      return json({
        network: "devnet",
        uptime_secs: 3600,
        block_producer: PUBLIC,
        coinbase_receiver: RECEIVER,
      });
    if (path === "/stats/block_producer") return json(recorded);
    if (path === "/snarker/control")
      return json({ allowed: false, enabled: false });
    return json({ error: "not here" }, 404);
  });
  render(
    <NodeClientProvider client={createNodeClient({ fetch })}>
      <NodeStatusProvider>
        <BlockProducerPage />
      </NodeStatusProvider>
    </NodeClientProvider>,
  );
  const label = await screen.findByText("Coinbase receiver");
  const row = label.closest("div")!.parentElement!;
  expect(row.textContent).toContain(RECEIVER.slice(0, 8));
  expect(screen.queryByText("The producer key")).toBeNull();
  expect(receiverFieldOrNull()).toBeNull();
});

test("when delegated stake counts: a link to the block producer docs", () => {
  state.keys = { key: held, saved: null };
  render(<BlockProducerPage />);
  const link = screen.getByRole("link", { name: /Block producer docs/ });
  expect(link.getAttribute("href")).toBe(PRODUCER_DOCS_URL);
  expect(screen.queryByText(/Why it takes two epochs/)).toBeNull();
  const [, path] = /^https:\/\/minanode\.org(\/docs\/[a-z-]+\/)$/.exec(
    PRODUCER_DOCS_URL,
  )!;
  expect(() =>
    readFileSync(
      `${import.meta.dirname}/../../../minanode-site/app${path}page.tsx`,
      "utf8",
    ),
  ).not.toThrow();
});

test("the one-role rule links its docs section", () => {
  state.keys = { key: held, saved: null };
  render(<BlockProducerPage />);
  const rule = document.querySelector("[data-one-role]")!;
  expect(rule.querySelector("a")?.getAttribute("href")).toBe(ONE_ROLE_DOCS_URL);
  expect(rule.textContent).toContain("Web node docs");
});
