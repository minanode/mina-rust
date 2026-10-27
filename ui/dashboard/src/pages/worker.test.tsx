// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { createNodeClient } from "@minanode/node-client";
import {
  NodeClientProvider,
  NodeStatusProvider,
} from "@minanode/node-client/react";
import { WorkerPage } from "./worker";

afterEach(cleanup);

// Public devnet provers, GET /snarker/included on 3610, 2026-10-24.
const KEY = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
const OTHER = "B62qpfgnUm7zVqi8MJHNB2m37rtgMNDbFNhC2DpMmmVpQt8x6gKv9Ww";
const offer = (fee: number, prover = KEY) => ({
  prover,
  bundles: 1,
  min_fee: fee,
  max_fee: fee,
});
const json = (value: unknown) => new Response(JSON.stringify(value));

function renderPage({
  offers = [offer(10_000_000, OTHER), offer(4_000_000)],
  market = true,
  control = {} as Record<string, unknown>,
  included = null as Promise<Response> | null,
} = {}) {
  const fetch = vi.fn(
    async (url: string | URL | Request, init?: RequestInit) => {
      expect(init?.method ?? "GET").toBe("GET");
      const path = String(url);
      if (path === "/snarker/control")
        return json({
          allowed: true,
          enabled: false,
          public_key: null,
          fee: null,
          account_creation_fee: 1_000_000_000,
          account_exists: null,
          worker: { kind: "None" },
          ...control,
        });
      if (path === "/snarker/included")
        return (
          included ??
          json({
            keys: [],
            entries: [],
            market: market ? { offers } : null,
          })
        );
      if (path === "/status") return json({ network: "devnet" });
      return json({});
    },
  );
  render(
    <NodeClientProvider client={createNodeClient({ fetch })}>
      <NodeStatusProvider>
        <WorkerPage />
      </NodeStatusProvider>
    </NodeClientProvider>,
  );
}

async function feeField() {
  return (await screen.findByRole("textbox", {
    name: "Fee per job, in MINA",
  })) as HTMLInputElement;
}

test("start fee follows the lowest pool offer, with its hint and new-account warning", async () => {
  renderPage();
  const fee = await feeField();
  await waitFor(() => expect(fee.value).toBe("0.004"));
  expect(
    screen.getByText(/Lowest offer in the pool now: 0.004 MINA/),
  ).toBeTruthy();
  expect(
    screen.getByText(
      /A key that has no account yet needs a fee of at least 1 MINA/,
    ),
  ).toBeTruthy();
});

test.each([true, false])(
  "no offers falls back to 0.1 MINA (market available: %s)",
  async market => {
    renderPage({ offers: [], market });
    expect((await feeField()).value).toBe("0.1");
    expect(screen.queryByText(/Lowest offer in the pool/)).toBeNull();
  },
);

test("a known key without an account is never prefilled below the account-creation fee", async () => {
  renderPage({ control: { public_key: KEY, account_exists: false } });
  expect((await feeField()).value).toBe("1");
  expect(
    await screen.findByText(/Lowest offer in the pool now: 0.004 MINA/),
  ).toBeTruthy();
});

test("zero-fee offers use the node's zero minimum for an existing account", async () => {
  renderPage({
    offers: [offer(0)],
    control: { public_key: KEY, account_exists: true },
  });
  const fee = await feeField();
  await waitFor(() => expect(fee.value).toBe("0"));
});

test("a running worker retains its configured fee", async () => {
  renderPage({
    control: {
      enabled: true,
      public_key: KEY,
      fee: 20_000_000,
      worker: { kind: "Idle" },
    },
  });
  expect((await feeField()).value).toBe("0.02");
});

test.each([false, true])(
  "late market data updates only an untouched fee (edited: %s)",
  async edited => {
    let resolve!: (value: Response) => void;
    renderPage({
      included: new Promise<Response>(r => {
        resolve = r;
      }),
    });
    const fee = await feeField();
    expect(fee.value).toBe("0.1");
    if (edited) fireEvent.change(fee, { target: { value: "0.008" } });
    resolve(json({ market: { offers: [offer(4_000_000)] } }));
    await screen.findByText(/Lowest offer in the pool now: 0.004 MINA/);
    expect(fee.value).toBe(edited ? "0.008" : "0.004");
  },
);

test("the header links to the SNARK worker docs in a new tab", async () => {
  renderPage();
  const link = await screen.findByRole("link", { name: /SNARK worker docs/ });
  expect(link.getAttribute("href")).toBe(
    "https://minanode.org/docs/snark-worker/",
  );
  expect(link.getAttribute("target")).toBe("_blank");
  expect(link.getAttribute("rel")).toContain("noopener");
});
