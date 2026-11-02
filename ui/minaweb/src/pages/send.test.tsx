// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

// Shapes only: not real keys.
const FROM = `B62q${"1".repeat(51)}`;
const TO = `B62q${"2".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;

const sendPayment = vi.fn();
/** What the node's pool and inclusion search answer. */
let poolHas: boolean | null = true;
/** The sender's account lookup: answered, loading, or failed. */
let senderLookup: "answered" | "loading" | "failed" = "answered";
const readAccount = vi.fn();
let inclusion: () => Promise<unknown> = async () => ({
  tipHeight: 10,
  depthSearched: 40,
  found: null,
});

vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../roles/hooks", () => {
  const account = (publicKey: string) => ({
    publicKey,
    balance: 100_000_000_000n,
    nonce: 4,
    delegate: null,
  });
  const node = {
    available: true,
    sendPayment: (...args: unknown[]) => sendPayment(...args),
    account: async (pk: string) => account(pk),
    inclusion: () => inclusion(),
    inPool: async () => {
      if (poolHas === null) throw new Error("busy");
      return poolHas;
    },
  };
  return {
    useKeys: () => ({
      key: { publicKey: FROM, privateKey: PRIVATE, origin: "generated" },
      saved: null,
    }),
    useKeyGlue: () => ({
      generate_account_key: () => null,
      account_public_key: () => FROM,
      account_public_key_valid: (k: string) => k === FROM || k === TO,
    }),
    useRoleNode: () => node,
    useAccount: (pk: string | null) =>
      pk === FROM && senderLookup !== "answered"
        ? {
            data: null,
            loading: senderLookup === "loading",
            error: senderLookup === "failed" ? new Error("busy") : null,
            updatedAt: null,
            refresh: readAccount,
          }
        : {
            data: pk ? account(pk) : null,
            loading: false,
            error: null,
            updatedAt: 1,
            refresh: () => {},
          },
  };
});

const { SendPage } = await import("./send");

beforeEach(() => {
  sendPayment.mockReset();
  sendPayment.mockResolvedValue({
    hash: "5JuTest",
    accepted: true,
    rejection: null,
  });
});
afterEach(cleanup);

const field = (id: string) => document.getElementById(id) as HTMLInputElement;
const type = (id: string, value: string) =>
  fireEvent.change(field(id), { target: { value } });

test("an edit after Review drops it: what is signed is what is shown", async () => {
  render(<SendPage />);
  type("send-to", TO);
  type("send-amount", "1");
  type("send-fee", "0.1");
  type("send-memo", "a private note");
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  const memo = await screen.findByText('"a private note"');
  expect(memo).toBeTruthy();
  expect(document.querySelector("[data-review-to]")?.textContent).toBe(TO);

  // The visitor clears the memo: the review is gone, nothing to sign.
  type("send-memo", "");
  expect(document.querySelector("[data-action=confirm-send]")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(document.querySelector("[data-review-memo]")?.textContent).toBe(
    "None",
  );
  await act(async () => {
    fireEvent.click(
      document.querySelector("[data-action=confirm-send]") as HTMLElement,
    );
  });
  expect(sendPayment).toHaveBeenCalledOnce();
  const [key, signed] = sendPayment.mock.calls[0];
  expect(key).toBe(PRIVATE);
  expect(signed).toEqual({
    from: FROM,
    to: TO,
    amount: 1_000_000_000n,
    fee: 100_000_000n,
    nonce: 4,
    memo: "",
  });
});

test("every field is in the confirmation, and each edit drops it", () => {
  render(<SendPage />);
  type("send-to", TO);
  type("send-amount", "2.5");
  type("send-fee", "0.01");
  for (const [id, value] of [
    ["send-to", FROM],
    ["send-amount", "3"],
    ["send-fee", "0.02"],
    ["send-memo", "x"],
  ] as const) {
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    const review = document.querySelector("[data-review]");
    if (review) {
      expect(review.textContent).toMatch(/Amount.*Fee.*Memo.*Nonce/);
    }
    type(id, value);
    expect(document.querySelector("[data-action=confirm-send]")).toBeNull();
  }
});

test("two payments accepted back to back are both kept", async () => {
  render(<SendPage />);
  // The store lives as long as the page: earlier tests' payments are in it.
  const before = document.querySelectorAll("[data-sent]").length;
  for (const [hash, amount] of [
    ["5JuOne", "1"],
    ["5JuTwo", "2"],
  ]) {
    sendPayment.mockResolvedValueOnce({
      hash,
      accepted: true,
      rejection: null,
    });
    type("send-to", TO);
    type("send-amount", amount);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await act(async () => {
      fireEvent.click(
        document.querySelector("[data-action=confirm-send]") as HTMLElement,
      );
    });
  }
  await waitFor(() =>
    expect(document.querySelectorAll("[data-sent]").length).toBe(before + 2),
  );
  // The second took the next nonce.
  const [first, second] = sendPayment.mock.calls.map(c => c[1].nonce);
  expect(second).toBe(first + 1);
});

test("the sent list comes back after the restart, with no secret in storage", async () => {
  render(<SendPage />);
  sendPayment.mockResolvedValueOnce({
    hash: "5JuKept",
    accepted: true,
    rejection: null,
  });
  type("send-to", TO);
  type("send-amount", "3");
  type("send-memo", "a private note");
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  await act(async () => {
    fireEvent.click(
      document.querySelector("[data-action=confirm-send]") as HTMLElement,
    );
  });
  const raw = sessionStorage.getItem("minaweb.sent") ?? "";
  expect(raw).toContain("5JuKept");
  expect(raw).not.toContain(PRIVATE);
  expect(raw).not.toContain("a private note");
  const sent = document.querySelectorAll("[data-sent]").length;
  const nonce = sendPayment.mock.calls.at(-1)![1].nonce;
  cleanup();

  // The role change's restart: the page's modules load again.
  vi.resetModules();
  const again = await import("./send");
  render(<again.SendPage />);
  await waitFor(() =>
    expect(document.querySelectorAll("[data-sent]").length).toBe(sent),
  );
  expect(screen.getByText("5JuKept", { exact: false })).toBeTruthy();
  // The next payment follows the kept one's nonce.
  sendPayment.mockResolvedValueOnce({
    hash: "5JuNext",
    accepted: true,
    rejection: null,
  });
  type("send-to", TO);
  type("send-amount", "1");
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  await act(async () => {
    fireEvent.click(
      document.querySelector("[data-action=confirm-send]") as HTMLElement,
    );
  });
  expect(sendPayment.mock.calls.at(-1)![1].nonce).toBe(nonce + 1);
});

/** A payment from before the restart, at the account's nonce (4). */
function keepFromBeforeRestart(nonce = 4) {
  sessionStorage.setItem(
    "minaweb.sent",
    JSON.stringify([
      {
        hash: "5JuBefore",
        from: FROM,
        to: TO,
        amount: "3000000000",
        fee: "100000000",
        nonce,
        sentAt: Date.now(),
        stage: "pending",
        detail: null,
        inclusion: null,
      },
    ]),
  );
}

test("Send waits while a payment from before the restart is checked", async () => {
  keepFromBeforeRestart();
  // The inclusion search doesn't answer yet.
  inclusion = () => new Promise(() => {});
  vi.resetModules();
  const { SendPage: Page } = await import("./send");
  render(<Page />);
  expect(document.querySelector("[data-nonce=checking]")).not.toBeNull();
  expect(
    screen.getByRole("button", { name: "Review" }).hasAttribute("disabled"),
  ).toBe(true);
  inclusion = async () => ({ tipHeight: 10, depthSearched: 40, found: null });
});

test("a payment missing after the restart gives its nonce to the next one", async () => {
  keepFromBeforeRestart();
  poolHas = false;
  vi.resetModules();
  const { SendPage: Page } = await import("./send");
  render(<Page />);
  await waitFor(() =>
    expect(document.querySelector("[data-nonce=missing]")).not.toBeNull(),
  );
  await act(async () =>
    fireEvent.click(
      document.querySelector("[data-action=refill]") as HTMLElement,
    ),
  );
  expect(field("send-to").value).toBe(TO);
  expect(field("send-amount").value).toBe("3");
  expect(field("send-fee").value).toBe("0.1");
  sendPayment.mockResolvedValueOnce({
    hash: "5JuAgain",
    accepted: true,
    rejection: null,
  });
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  await act(async () => {
    fireEvent.click(
      document.querySelector("[data-action=confirm-send]") as HTMLElement,
    );
  });
  // The same nonce, not one past it.
  expect(sendPayment.mock.calls.at(-1)![1].nonce).toBe(4);
  await waitFor(() =>
    expect(document.querySelector("[data-nonce=missing]")).toBeNull(),
  );
  poolHas = true;
});

const confirm = async () =>
  act(async () => {
    fireEvent.click(
      document.querySelector("[data-action=confirm-send]") as HTMLElement,
    );
  });

async function restartPage() {
  vi.resetModules();
  const { SendPage: Page } = await import("./send");
  render(<Page />);
}

test("while the account loads, a kept payment at nonce 0 still holds Send", async () => {
  keepFromBeforeRestart(0);
  senderLookup = "loading";
  inclusion = () => new Promise(() => {});
  try {
    await restartPage();
    type("send-to", TO);
    type("send-amount", "1");
    const review = screen.getByRole("button", { name: "Review" });
    expect(review.hasAttribute("disabled")).toBe(true);
    fireEvent.click(review);
    expect(document.querySelector("[data-action=confirm-send]")).toBeNull();
    expect(sendPayment).not.toHaveBeenCalled();
  } finally {
    senderLookup = "answered";
    inclusion = async () => ({ tipHeight: 10, depthSearched: 40, found: null });
  }
});

test("an account that couldn't be read says so, with Retry", async () => {
  sessionStorage.clear();
  senderLookup = "failed";
  readAccount.mockClear();
  try {
    await restartPage();
    expect(document.querySelector("[data-nonce=unknown]")).not.toBeNull();
    // Not taken for an unfunded key.
    expect(screen.queryByText("This key has no devnet MINA yet")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Review" }).hasAttribute("disabled"),
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(readAccount).toHaveBeenCalledOnce();
  } finally {
    senderLookup = "answered";
  }
});

test("a check that fails keeps Send waiting, and Retry looks again", async () => {
  keepFromBeforeRestart();
  poolHas = null;
  try {
    await restartPage();
    await waitFor(() =>
      expect(
        document.querySelector("[data-nonce=check-failed]"),
      ).not.toBeNull(),
    );
    expect(
      screen.getByRole("button", { name: "Review" }).hasAttribute("disabled"),
    ).toBe(true);
    poolHas = true;
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Retry" })),
    );
    await waitFor(() =>
      expect(document.querySelector("[data-nonce]")).toBeNull(),
    );
    expect(
      screen.getByRole("button", { name: "Review" }).hasAttribute("disabled"),
    ).toBe(false);
  } finally {
    poolHas = true;
  }
});

test("a missing payment back in the pool before signing isn't replaced", async () => {
  keepFromBeforeRestart();
  poolHas = false;
  try {
    await restartPage();
    await waitFor(() =>
      expect(document.querySelector("[data-nonce=missing]")).not.toBeNull(),
    );
    fireEvent.click(
      document.querySelector("[data-action=refill]") as HTMLElement,
    );
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    // It comes back before Sign and send.
    poolHas = true;
    await confirm();
    expect(sendPayment).not.toHaveBeenCalled();
    expect(document.querySelector("[data-send-outcome]")?.textContent).toMatch(
      /back in the pool/,
    );
    const { sentStore } = await import("../roles/sent");
    expect(sentStore.getSnapshot()[0]).toMatchObject({
      hash: "5JuBefore",
      stage: "pending",
    });
    expect(sentStore.getSnapshot()[0].check).toBeUndefined();
  } finally {
    poolHas = true;
  }
});

test("the pool's Duplicate for the same payment keeps its nonce", async () => {
  keepFromBeforeRestart();
  poolHas = false;
  try {
    await restartPage();
    await waitFor(() =>
      expect(document.querySelector("[data-nonce=missing]")).not.toBeNull(),
    );
    fireEvent.click(
      document.querySelector("[data-action=refill]") as HTMLElement,
    );
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    // It reaches the pool after the look before signing: the signer's
    // answer is the only sign.
    sendPayment.mockResolvedValueOnce({
      hash: "5JuBefore",
      accepted: false,
      rejection: "Duplicate",
    });
    await confirm();
    expect(sendPayment).toHaveBeenCalledOnce();
    expect(sendPayment.mock.calls[0][1].nonce).toBe(4);
    const { sentStore, nonceFor } = await import("../roles/sent");
    const list = sentStore.getSnapshot();
    expect(list.filter(s => s.hash === "5JuBefore")).toHaveLength(1);
    expect(list[0]).toMatchObject({ hash: "5JuBefore", stage: "pending" });
    expect(list[0].check).toBeUndefined();
    expect(
      nonceFor(
        {
          publicKey: FROM,
          nonce: 4,
          balance: 100_000_000_000n,
          delegate: null,
        },
        list,
      ),
    ).toEqual({ kind: "ready", nonce: 5 });
  } finally {
    poolHas = true;
  }
});
