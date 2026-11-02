// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  createNodeClient,
  createWebNodeTransport,
  MINA_TOKEN_ID,
  type WebNodeRpc,
} from "@minanode/node-client";
import { NodeClientProvider } from "@minanode/node-client/react";

const state = vi.hoisted(() => ({
  publicKey: `B62q${"1".repeat(51)}`,
  get: vi.fn(),
  rpc: {} as Record<string, unknown>,
}));
vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../app/boot-context", () => ({
  useBoot: () => ({ rpc: state.rpc, role: { kind: "viewer" } }),
}));
vi.mock("../roles/hooks", async importOriginal => ({
  ...(await importOriginal<typeof import("../roles/hooks")>()),
  useKeys: () => ({
    key: {
      publicKey: state.publicKey,
      privateKey: "synthetic",
      origin: "generated",
    },
    saved: null,
  }),
  useKeyGlue: () => ({ account_public_key_valid: () => true }),
}));
vi.mock("@minanode/node-client/react", async importOriginal => ({
  ...(await importOriginal<typeof import("@minanode/node-client/react")>()),
  useNodeStatus: () => ({ status: { data: { bestTip: { height: 100 } } } }),
}));
const { KeyPage } = await import("./key");
const { SendPage } = await import("./send");
const { AccountsPage } = await import("./accounts");

beforeEach(() => {
  state.get.mockReset().mockImplementation(async (public_key: string) => [
    { public_key, token_id: "other", balance: "1", nonce: 100 },
    {
      public_key,
      token_id: MINA_TOKEN_ID,
      balance: "9007199254740993",
      nonce: 4,
      delegate: public_key,
    },
  ]);
  state.rpc = {
    state: () => ({}),
    stats: () => ({}),
    roles: () => ({}),
    ledger: () => ({
      latest: () => ({ accounts: () => ({ get: state.get }) }),
    }),
    transaction_pool: () => ({ get: async () => [] }),
  };
});
afterEach(cleanup);

test("key screen displays the exact MINA summary through its real account hook", async () => {
  render(<KeyPage />);
  await screen.findByText("9,007,199.254740993");
  expect(state.get).toHaveBeenCalledWith(state.publicKey);
  expect(screen.getByText("4")).toBeTruthy();
});

test("accounts page uses the same summary through the web transport", async () => {
  const client = createNodeClient({
    transport: createWebNodeTransport(state.rpc as unknown as WebNodeRpc),
  });
  render(
    <NodeClientProvider client={client}>
      <AccountsPage publicKey={state.publicKey} />
    </NodeClientProvider>,
  );
  await waitFor(() =>
    expect(
      document.querySelector("[data-balance]")?.getAttribute("data-balance"),
    ).toBe("9007199254740993"),
  );
  expect(state.get).toHaveBeenCalledWith(state.publicKey);
});

test("send validates the last nanomina above 2^53 using the MINA account", async () => {
  render(<SendPage />);
  await waitFor(() => expect(state.get).toHaveBeenCalledWith(state.publicKey));
  const change = (id: string, value: string) =>
    fireEvent.change(document.getElementById(id)!, { target: { value } });
  change("send-to", `B62q${"2".repeat(51)}`);
  change("send-fee", "0.001");
  change("send-amount", "9007199.253740993");
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "Review" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(document.querySelector("[data-review]")?.textContent).toMatch(
    /Nonce4/,
  );
  change("send-amount", "9007199.253740994");
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(document.querySelector("[data-review]")).toBeNull();
  expect(
    screen.getByText("More than this key's balance, with the fee."),
  ).toBeTruthy();
});
