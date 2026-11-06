// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  carryHeld,
  takeHandoff,
  type HandoffDb,
  type HandoffEntry,
} from "../roles/handoff";
import type { KeyFileGlue, KeyGlue } from "../roles/keys";
import { installStorage, MemoryStorage } from "../roles/test-storage";

// Shapes only: the glue below is a stand-in, these are not real keys.
const PUBLIC = `B62q${"1".repeat(51)}`;
const PRIVATE = `EK${"3".repeat(50)}`;
const PASSWORD = "the file's password";
const FILE = JSON.stringify({
  box_primitive: "xsalsa20poly1305",
  pw_primitive: "argon2i",
  nonce: "n",
  pwsalt: "s",
  pwdiff: [134217728, 6],
  ciphertext: "c",
});

const state = vi.hoisted(() => ({
  fromFile: vi.fn(),
  toFile: vi.fn(),
}));
const glue: KeyGlue = {
  generate_account_key: () => null,
  account_public_key: () => PUBLIC,
  account_public_key_valid: k => k === PUBLIC,
};
const fileGlue: KeyFileGlue = {
  account_key_from_file: (...args) => state.fromFile(...args),
  account_key_to_file: (...args) => state.toFile(...args),
};

vi.mock("../config", () => ({ CONFIG: { network: "devnet" } }));
vi.mock("../app/boot-context", () => ({
  useBoot: () => ({ rpc: null, glue: null, role: { kind: "viewer" } }),
}));
vi.mock("../roles/hooks", async original => ({
  ...(await original<typeof import("../roles/hooks")>()),
  useKeyGlue: () => glue,
  useKeyFileGlue: () => fileGlue,
  useAccount: () => ({
    data: null,
    loading: false,
    error: null,
    updatedAt: 1,
    refresh: () => {},
  }),
}));

const { keyStore } = await import("../roles/keys");
const { restartWithKey } = await import("../roles/restart");
const { KeyPage } = await import("./key");

let local: MemoryStorage;
let session: MemoryStorage;
beforeEach(() => {
  local = new MemoryStorage();
  session = new MemoryStorage();
  installStorage("localStorage", local);
  installStorage("sessionStorage", session);
  keyStore.forget();
  state.fromFile.mockReset();
  state.fromFile.mockImplementation(async (file: string, password: string) => {
    if (file !== FILE) throw new Error("not-a-key-file");
    if (password !== PASSWORD) throw new Error("wrong-password");
    return { publicKey: PUBLIC, privateKey: PRIVATE };
  });
  state.toFile.mockReset();
});
afterEach(cleanup);

const field = (id: string) =>
  document.getElementById(id) as HTMLInputElement | HTMLTextAreaElement;
const type = (id: string, value: string) =>
  fireEvent.change(field(id), { target: { value } });
const importButton = () =>
  screen.getByRole("button", {
    name: /Decrypt and import|Decrypting/,
  }) as HTMLButtonElement;
const error = () =>
  document.querySelector("[data-import-key-file-error]")?.textContent ?? null;

function openKeyFileImport({ acknowledge = true } = {}) {
  render(<KeyPage />);
  fireEvent.click(
    screen.getByRole("button", { name: "Import a devnet key instead" }),
  );
  if (acknowledge) fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Key file" }));
}

async function submit() {
  await act(async () => {
    fireEvent.submit(document.querySelector("[data-import-key-file]")!);
  });
}

/** Everything this test's page wrote, and the page's text. */
const dump = (s: Storage) =>
  [...Array(s.length)].map((_, i) => [s.key(i), s.getItem(s.key(i)!)]);
const everything = () =>
  JSON.stringify([dump(local), dump(session)]) + document.body.innerHTML;

describe("importing a key file", () => {
  test("the real-funds warning gates it", async () => {
    openKeyFileImport({ acknowledge: false });
    expect(document.querySelector("[data-import-warning]")).toBeTruthy();
    for (const id of [
      "import-key-file",
      "import-key-file-text",
      "import-key-file-password",
    ]) {
      expect(field(id).disabled).toBe(true);
    }
    expect(importButton().disabled).toBe(true);
    // Even a forced submit does nothing.
    await submit();
    expect(state.fromFile).not.toHaveBeenCalled();
    expect(keyStore.getSnapshot().key).toBeNull();
  });

  test("a pasted file and its password give the key, shown only as its public key", async () => {
    openKeyFileImport();
    type("import-key-file-text", FILE);
    type("import-key-file-password", PASSWORD);
    expect(importButton().disabled).toBe(false);
    await submit();
    expect(state.fromFile).toHaveBeenCalledWith(FILE, PASSWORD);
    expect(keyStore.getSnapshot().key).toEqual({
      publicKey: PUBLIC,
      privateKey: PRIVATE,
      origin: "imported",
    });
    await screen.findByText("Imported into this tab.", { exact: false });
    expect(
      document
        .querySelector("[data-key-public]")
        ?.getAttribute("data-key-public"),
    ).toBe(PUBLIC);
    // Not on screen, not in storage, and nor is the password.
    expect(everything()).not.toContain(PRIVATE);
    expect(everything()).not.toContain(PASSWORD);
    expect(local.length + session.length).toBe(0);
  });

  test("a picked file is read, then imported", async () => {
    openKeyFileImport();
    const file = new File([FILE], "producer-key");
    await act(async () => {
      fireEvent.change(field("import-key-file"), {
        target: { files: [file] },
      });
    });
    await waitFor(() => expect(field("import-key-file-text").value).toBe(FILE));
    type("import-key-file-password", PASSWORD);
    await submit();
    expect(keyStore.getSnapshot().key?.publicKey).toBe(PUBLIC);
  });

  test("a wrong password says so, and the password is cleared", async () => {
    openKeyFileImport();
    type("import-key-file-text", FILE);
    type("import-key-file-password", "not it");
    await submit();
    expect(error()).toBe("Wrong password, or the file is damaged.");
    expect(field("import-key-file-password").value).toBe("");
    expect(keyStore.getSnapshot().key).toBeNull();
  });

  test.each([
    ["not json", "That isn't a Mina encrypted key file."],
    [
      PUBLIC,
      "That is the public key (the .pub file). Choose the key file itself.",
    ],
    [
      PRIVATE,
      "That is a raw private key, not a key file. Import it under Private key.",
    ],
  ])("a bad file is named: %s", async (text, message) => {
    openKeyFileImport();
    type("import-key-file-text", text);
    type("import-key-file-password", PASSWORD);
    await submit();
    expect(error()).toBe(message);
    expect(error()).not.toContain(PASSWORD);
    expect(keyStore.getSnapshot().key).toBeNull();
  });

  test("a file too large to be a key file isn't read", async () => {
    openKeyFileImport();
    const big = new File(["x".repeat(20_000)], "big.json");
    await act(async () => {
      fireEvent.change(field("import-key-file"), { target: { files: [big] } });
    });
    expect(error()).toBe("That isn't a Mina encrypted key file.");
    expect(field("import-key-file-text").value).toBe("");
  });

  test.each([
    [
      "unsupported",
      "That key file's encryption isn't supported: the page reads xsalsa20poly1305 with argon2i.",
    ],
    [
      "not-an-account-key",
      "That key file holds another kind of key, such as a libp2p key, not an account key.",
    ],
    [
      "too-expensive",
      "That key file asks for more memory to decrypt than this page allows (256 MiB).",
    ],
    ["something else", "The page couldn't read that key file."],
  ])("the WASM's %s is named", async (code, message) => {
    state.fromFile.mockRejectedValue(new Error(code));
    openKeyFileImport();
    type("import-key-file-text", FILE);
    type("import-key-file-password", PASSWORD);
    await submit();
    expect(error()).toBe(message);
  });
});

/** IndexedDB as a Map, as handoff.test.ts has it. */
class MemoryDb implements HandoffDb {
  map = new Map<string, HandoffEntry>();
  async put(entry: HandoffEntry) {
    this.map.set(entry.id, entry);
  }
  async get(id: string) {
    return this.map.get(id) ?? null;
  }
  async take(id: string) {
    const entry = this.map.get(id) ?? null;
    this.map.delete(id);
    return entry;
  }
  async delete(id: string) {
    this.map.delete(id);
  }
  async list() {
    return [...this.map.values()].map(e => ({ id: e.id, at: e.at }));
  }
}

test("an imported key file's key crosses a role change, as a raw import does", async () => {
  openKeyFileImport();
  type("import-key-file-text", FILE);
  type("import-key-file-password", PASSWORD);
  await submit();
  const held = keyStore.getSnapshot().key!;
  const deps = { db: new MemoryDb(), now: () => 1_800_000_000_000 };
  const go = vi.fn();
  await restartWithKey(
    { kind: "worker", publicKey: PUBLIC, fee: "1" },
    held,
    null,
    go,
    key => carryHeld(key, deps),
  );
  expect(go).toHaveBeenCalledOnce();
  // The next load.
  keyStore.forget();
  const back = await takeHandoff(deps);
  expect(back).toEqual(held);
  keyStore.hold(back!);
  expect(keyStore.getSnapshot().key).toEqual({
    publicKey: PUBLIC,
    privateKey: PRIVATE,
    origin: "imported",
  });
});

describe("exporting a key file", () => {
  test("downloads the WASM's file under the chosen password", async () => {
    state.toFile.mockResolvedValue(FILE);
    const created: Blob[] = [];
    URL.createObjectURL = vi.fn((blob: Blob) => {
      created.push(blob);
      return "blob:test";
    });
    URL.revokeObjectURL = vi.fn();
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});
    keyStore.hold({
      publicKey: PUBLIC,
      privateKey: PRIVATE,
      origin: "imported",
    });
    render(<KeyPage />);
    type("export-password", "a new password");
    type("export-again", "a new password");
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Download key file" }),
      );
    });
    expect(state.toFile).toHaveBeenCalledWith(PRIVATE, "a new password");
    expect(click).toHaveBeenCalledOnce();
    expect(await created[0].text()).toBe(FILE);
    expect(document.querySelector("[data-export-done]")).toBeTruthy();
    expect(field("export-password").value).toBe("");
    click.mockRestore();
  });

  test("needs the same password twice, of ten characters", async () => {
    keyStore.hold({
      publicKey: PUBLIC,
      privateKey: PRIVATE,
      origin: "imported",
    });
    render(<KeyPage />);
    const button = () =>
      screen.getByRole("button", {
        name: "Download key file",
      }) as HTMLButtonElement;
    type("export-password", "short");
    type("export-again", "short");
    expect(button().disabled).toBe(true);
    type("export-password", "a new password");
    type("export-again", "another password");
    await act(async () => {
      fireEvent.click(button());
    });
    await screen.findByText("The two passwords differ.");
    expect(state.toFile).not.toHaveBeenCalled();
  });
});

describe("the saved copy of a held key", () => {
  test("is deleted only after a confirmation", async () => {
    keyStore.hold({
      publicKey: PUBLIC,
      privateKey: PRIVATE,
      origin: "imported",
    });
    await keyStore.remember("correct horse battery");
    render(<KeyPage />);
    const del = () =>
      screen.getByRole("button", { name: "Delete the saved key" });

    fireEvent.click(del());
    expect(
      screen.getByRole("group", { name: "Confirm deleting the saved copy" }),
    ).toBeTruthy();
    expect(keyStore.getSnapshot().saved).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(keyStore.getSnapshot().saved).not.toBeNull();

    fireEvent.click(del());
    fireEvent.click(screen.getByRole("button", { name: "Delete it" }));
    expect(keyStore.getSnapshot().saved).toBeNull();
    expect(local.getItem("minaweb.key")).toBeNull();
    // The key itself stays held in the tab.
    expect(keyStore.getSnapshot().key?.publicKey).toBe(PUBLIC);
  });
});
