/**
 * The tab's devnet key: made here by default, or imported behind a plain
 * warning; its public key, balance and nonce; a backup of the private key
 * on request; and, if the visitor asks, a copy saved encrypted in this
 * browser.
 */

import * as React from "react";
import { Eye, EyeOff, KeyRound, ShieldAlert, Sparkles } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Textarea } from "@minanode/design-system/components/textarea";
import { Amount } from "@minanode/design-system/patterns/amount";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { formatCount, formatMina } from "@minanode/node-client";

import { useBoot } from "../app/boot-context";
import {
  exportKeyFile,
  generateKey,
  importKey,
  importKeyFile,
  KEY_FILE_MAX_BYTES,
  keyStore,
  MIN_PASSPHRASE,
  passphraseProblem,
  type HeldKey,
  type KeyFileGlue,
  type KeyGlue,
} from "../roles/keys";
import {
  useAccount,
  useKeyFileGlue,
  useKeyGlue,
  useKeys,
} from "../roles/hooks";
import { ROLE_LABELS } from "../roles/role";
import { FAUCET_URL, RolesGate } from "./roles-common";

/** The docs section on the key: where it is made, kept and sent. */
export const KEY_DOCS_URL =
  "https://minanode.org/docs/web-node-connections/#your-key";

export function KeyPage() {
  const glue = useKeyGlue();
  const { key, saved } = useKeys();
  return (
    <>
      <PageHeader
        eyebrow="devnet"
        title="Your key"
        description={
          <>
            A devnet account key for this tab.{" "}
            <ExternalLink href={KEY_DOCS_URL}>Web node docs</ExternalLink>
          </>
        }
      />
      <RolesGate>
        <div className="flex max-w-3xl flex-col gap-6">
          {!glue ? (
            <Alert variant="caution">
              <ShieldAlert aria-hidden />
              <AlertTitle>This node can't make keys</AlertTitle>
              <AlertDescription>
                <p>
                  Its WASM is older than this page. Reload to get both again.
                </p>
              </AlertDescription>
            </Alert>
          ) : key ? (
            <HeldKeyView glue={glue} />
          ) : (
            <NoKey glue={glue} hasSaved={saved !== null} />
          )}
        </div>
      </RolesGate>
    </>
  );
}

function NoKey({ glue, hasSaved }: { glue: KeyGlue; hasSaved: boolean }) {
  const [error, setError] = React.useState<string | null>(null);
  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Sparkles aria-hidden className="size-4 text-brand-text" />
            Generate a new key
          </CardTitle>
          <CardDescription>
            Back it up before you close the tab, or it is gone.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Button
            type="button"
            className="w-fit"
            data-action="generate-key"
            onClick={() => {
              try {
                keyStore.hold(generateKey(glue));
                setError(null);
              } catch (e) {
                setError(e instanceof Error ? e.message : String(e));
              }
            }}
          >
            <KeyRound aria-hidden />
            Generate a new key
          </Button>
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
        </CardContent>
      </Card>
      {hasSaved ? <UnlockSaved glue={glue} /> : null}
      <ImportKey glue={glue} />
    </>
  );
}

function UnlockSaved({ glue }: { glue: KeyGlue }) {
  const { saved } = useKeys();
  const [passphrase, setPassphrase] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  if (!saved) return null;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Unlock the key saved in this browser</CardTitle>
        <CardDescription>
          Saved encrypted with your passphrase. Public key:{" "}
          <Hash value={saved.publicKey} label="saved public key" />
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <form
          className="flex flex-col gap-3"
          onSubmit={async event => {
            event.preventDefault();
            setBusy(true);
            try {
              await keyStore.unlockSaved(passphrase, glue);
              setError(null);
            } catch (e) {
              setError(e instanceof Error ? e.message : "Wrong passphrase.");
            } finally {
              setBusy(false);
              setPassphrase("");
            }
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="saved-passphrase">Passphrase</Label>
            <Input
              id="saved-passphrase"
              type="password"
              autoComplete="current-password"
              value={passphrase}
              onChange={e => setPassphrase(e.target.value)}
              aria-invalid={error ? true : undefined}
            />
            {error ? <p className="text-sm text-destructive">{error}</p> : null}
          </div>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={busy || !passphrase}>
              {busy ? "Unlocking…" : "Unlock"}
            </Button>
            <Button
              type="button"
              variant="ghost"
              onClick={() => setConfirmDelete(true)}
            >
              Delete the saved key
            </Button>
          </div>
        </form>
        {confirmDelete ? (
          <InlineConfirm
            label="Confirm deleting the saved key"
            message="This deletes the encrypted copy from this browser. Without a backup, the key and its devnet MINA are gone."
            confirmLabel="Delete it"
            onConfirm={() => {
              keyStore.deleteSaved();
              setConfirmDelete(false);
            }}
            onCancel={() => setConfirmDelete(false)}
          />
        ) : null}
      </CardContent>
    </Card>
  );
}

function ImportKey({ glue }: { glue: KeyGlue }) {
  const fileGlue = useKeyFileGlue();
  const [open, setOpen] = React.useState(false);
  const [understood, setUnderstood] = React.useState(false);
  const [kind, setKind] = React.useState<"raw" | "file">("raw");
  if (!open) {
    return (
      <div>
        <Button type="button" variant="outline" onClick={() => setOpen(true)}>
          Import a devnet key instead
        </Button>
      </div>
    );
  }
  const close = () => setOpen(false);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Import a devnet key</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <Alert variant="destructive" data-import-warning>
          <ShieldAlert aria-hidden />
          <AlertTitle>Never paste a key that holds real MINA</AlertTitle>
          <AlertDescription>
            <p>
              A Mina key is the same on every network: this key controls the
              same account on mainnet. Never paste a key that holds real MINA
              into any web page, this one included. Import only a key you made
              for devnet.
            </p>
          </AlertDescription>
        </Alert>
        <label className="flex items-start gap-2 text-sm text-foreground">
          <input
            type="checkbox"
            className="mt-1"
            checked={understood}
            onChange={e => setUnderstood(e.target.checked)}
          />
          This key holds no mainnet MINA, and I made it for devnet.
        </label>
        {fileGlue ? (
          <SegmentedControl
            label="What to import"
            options={[
              { value: "raw", label: "Private key" },
              { value: "file", label: "Key file" },
            ]}
            value={kind}
            onValueChange={setKind}
          />
        ) : null}
        {kind === "file" && fileGlue ? (
          <ImportKeyFile
            glue={fileGlue}
            understood={understood}
            onCancel={close}
          />
        ) : (
          <ImportRawKey glue={glue} understood={understood} onCancel={close} />
        )}
      </CardContent>
    </Card>
  );
}

function ImportRawKey({
  glue,
  understood,
  onCancel,
}: {
  glue: KeyGlue;
  understood: boolean;
  onCancel: () => void;
}) {
  const [text, setText] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={event => {
        event.preventDefault();
        if (!understood) return;
        try {
          keyStore.hold(importKey(glue, text));
          setText("");
          setError(null);
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }}
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="import-key">Private key (EK…)</Label>
        <Textarea
          id="import-key"
          value={text}
          disabled={!understood}
          onChange={e => setText(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          rows={2}
          className="font-mono"
          aria-invalid={error ? true : undefined}
        />
        {error ? <p className="text-sm text-destructive">{error}</p> : null}
      </div>
      <div className="flex gap-2">
        <Button type="submit" disabled={!understood || !text.trim()}>
          Import
        </Button>
        <Button
          type="button"
          variant="ghost"
          onClick={() => {
            setText("");
            onCancel();
          }}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** A Mina encrypted key file (OCaml's, or `mina wallet generate`'s),
 * picked or pasted, and its password. The WASM decrypts it off the page's
 * thread; the private key goes to the key store only. */
function ImportKeyFile({
  glue,
  understood,
  onCancel,
}: {
  glue: KeyFileGlue;
  understood: boolean;
  onCancel: () => void;
}) {
  const [text, setText] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const disabled = !understood || busy;
  return (
    <form
      className="flex flex-col gap-3"
      data-import-key-file
      onSubmit={async event => {
        event.preventDefault();
        if (!understood) return;
        setBusy(true);
        try {
          const key = await importKeyFile(glue, text, password);
          setText("");
          setError(null);
          keyStore.hold(key);
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
        } finally {
          setPassword("");
          setBusy(false);
        }
      }}
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="import-key-file">Key file</Label>
        <Input
          id="import-key-file"
          type="file"
          disabled={disabled}
          className="h-auto"
          onChange={async e => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (!file) return;
            if (file.size > KEY_FILE_MAX_BYTES) {
              setError("That isn't a Mina encrypted key file.");
              return;
            }
            try {
              setText(await file.text());
              setError(null);
            } catch {
              setError("The browser couldn't read that file.");
            }
          }}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="import-key-file-text">Or paste its contents</Label>
        <Textarea
          id="import-key-file-text"
          value={text}
          disabled={disabled}
          onChange={e => setText(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          rows={4}
          className="font-mono text-xs"
          placeholder='{"box_primitive":"xsalsa20poly1305",…}'
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="import-key-file-password">Password</Label>
        <Input
          id="import-key-file-password"
          type="password"
          autoComplete="off"
          value={password}
          disabled={disabled}
          onChange={e => setPassword(e.target.value)}
          aria-invalid={error ? true : undefined}
        />
        {error ? (
          <p className="text-sm text-destructive" data-import-key-file-error>
            {error}
          </p>
        ) : null}
      </div>
      <div className="flex gap-2">
        <Button type="submit" disabled={disabled || !text.trim() || !password}>
          {busy ? "Decrypting…" : "Decrypt and import"}
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={() => {
            setText("");
            setPassword("");
            onCancel();
          }}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

function HeldKeyView({ glue }: { glue: KeyGlue }) {
  const { key, saved } = useKeys();
  const boot = useBoot();
  const account = useAccount(key?.publicKey ?? null);
  const fileGlue = useKeyFileGlue();
  const [confirmForget, setConfirmForget] = React.useState(false);
  if (!key) return null;
  void glue;
  const a = account.data;
  const roleUsesKey =
    boot.role.kind !== "viewer" && boot.role.publicKey === key.publicKey;
  return (
    <>
      <Card data-key-public={key.publicKey}>
        <CardHeader>
          <CardTitle>Public key</CardTitle>
          <CardDescription>
            {key.origin === "generated"
              ? "Made in this tab."
              : "Imported into this tab."}{" "}
            Share it to receive devnet MINA.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Hash
            value={key.publicKey}
            truncate={false}
            label="public key"
            className="text-sm wrap-anywhere"
          />
        </CardContent>
      </Card>

      <KeyValueList
        title="Account on devnet"
        rows={
          account.loading
            ? [{ label: "Balance", value: "Looking it up…" }]
            : a
              ? [
                  {
                    label: "Balance",
                    value: <Amount unit="MINA" value={formatMina(a.balance)} />,
                    tone: "brand",
                  },
                  { label: "Nonce", value: formatCount(a.nonce) },
                  {
                    label: "Delegates to",
                    value: a.delegate ? (
                      a.delegate === a.publicKey ? (
                        "Itself"
                      ) : (
                        <Hash value={a.delegate} label="delegate" />
                      )
                    ) : (
                      "–"
                    ),
                  },
                ]
              : [
                  {
                    label: "Balance",
                    value: "No account yet",
                  },
                ]
        }
        footer={
          a ? (
            "At this node's best tip."
          ) : (
            <>
              The account exists once it receives MINA.{" "}
              <ExternalLink href={FAUCET_URL}>
                Get devnet MINA from the faucet
              </ExternalLink>{" "}
              for this public key.
            </>
          )
        }
      />

      <Backup
        privateKey={key.privateKey}
        generated={key.origin === "generated"}
      />
      <SaveEncrypted savedHere={saved?.publicKey === key.publicKey} />
      {fileGlue ? <ExportKeyFile glue={fileGlue} heldKey={key} /> : null}

      <div className="flex flex-col gap-3">
        <div>
          <Button
            type="button"
            variant="outline"
            onClick={() => setConfirmForget(true)}
          >
            Forget the key in this tab
          </Button>
        </div>
        {confirmForget ? (
          <InlineConfirm
            label="Confirm forgetting the key"
            message={
              <>
                The page drops the key.{" "}
                {saved?.publicKey === key.publicKey
                  ? "The encrypted copy in this browser stays."
                  : "Without a backup, it and its devnet MINA are gone."}
                {roleUsesKey
                  ? ` The ${ROLE_LABELS[boot.role.kind].toLowerCase()} keeps running until the tab closes or you stop it.`
                  : ""}
              </>
            }
            confirmLabel="Forget it"
            onConfirm={() => {
              keyStore.forget();
              setConfirmForget(false);
            }}
            onCancel={() => setConfirmForget(false)}
          />
        ) : null}
      </div>
    </>
  );
}

function Backup({
  privateKey,
  generated,
}: {
  privateKey: string;
  generated: boolean;
}) {
  const [shown, setShown] = React.useState(false);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Private key</CardTitle>
        <CardDescription>
          {generated
            ? "Only this tab has it. Write it down or copy it somewhere safe, or save it encrypted below: closing the tab loses it otherwise."
            : "Anyone with it controls the account, on devnet and on mainnet."}
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {shown ? (
          <code
            className="block rounded-md border bg-muted px-3 py-2 font-mono text-sm wrap-anywhere"
            data-private-key-shown
          >
            {privateKey}
          </code>
        ) : (
          <p className="font-mono text-sm text-muted-foreground">EK••••••••</p>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setShown(s => !s)}
            data-action="reveal-key"
          >
            {shown ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
            {shown ? "Hide it" : "Show it"}
          </Button>
          <CopyButton text={privateKey} label="private key" />
        </div>
      </CardContent>
    </Card>
  );
}

function SaveEncrypted({ savedHere }: { savedHere: boolean }) {
  const [passphrase, setPassphrase] = React.useState("");
  const [again, setAgain] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  if (savedHere) {
    return (
      <Alert variant="brand">
        <KeyRound aria-hidden />
        <AlertTitle>Saved in this browser, encrypted</AlertTitle>
        <AlertDescription>
          <p>
            Its encrypted copy stays here until you delete it. The passphrase
            unlocks it in a new tab.
          </p>
          {confirmDelete ? (
            <InlineConfirm
              label="Confirm deleting the saved copy"
              message="This deletes the encrypted copy from this browser. The key stays in this tab until you close it."
              confirmLabel="Delete it"
              onConfirm={() => {
                keyStore.deleteSaved();
                setConfirmDelete(false);
              }}
              onCancel={() => setConfirmDelete(false)}
            />
          ) : (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="mt-2 w-fit"
              onClick={() => setConfirmDelete(true)}
            >
              Delete the saved copy
            </Button>
          )}
        </AlertDescription>
      </Alert>
    );
  }
  const problem = passphraseProblem(passphrase);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Save it in this browser (optional)</CardTitle>
        <CardDescription>
          Anyone with this device and the passphrase can use it.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className="flex flex-col gap-3"
          onSubmit={async event => {
            event.preventDefault();
            if (passphrase !== again) {
              setError("The two passphrases differ.");
              return;
            }
            setBusy(true);
            try {
              await keyStore.remember(passphrase);
              setError(null);
              setPassphrase("");
              setAgain("");
            } catch (e) {
              setError(e instanceof Error ? e.message : String(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="save-passphrase">Passphrase</Label>
              <Input
                id="save-passphrase"
                type="password"
                autoComplete="new-password"
                value={passphrase}
                onChange={e => setPassphrase(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="save-again">The same again</Label>
              <Input
                id="save-again"
                type="password"
                autoComplete="new-password"
                value={again}
                onChange={e => setAgain(e.target.value)}
              />
            </div>
          </div>
          <p className="text-sm text-muted-foreground">
            At least {MIN_PASSPHRASE} characters.
          </p>
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          <Button
            type="submit"
            variant="outline"
            className="w-fit"
            disabled={busy || problem !== null || !again}
          >
            {busy ? "Encrypting…" : "Save encrypted"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

/** The held key as a Mina encrypted key file, for a native node: made in
 * the WASM under a password the visitor chooses, then downloaded. */
function ExportKeyFile({
  glue,
  heldKey,
}: {
  glue: KeyFileGlue;
  heldKey: HeldKey;
}) {
  const [password, setPassword] = React.useState("");
  const [again, setAgain] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [done, setDone] = React.useState(false);
  const problem = passphraseProblem(password);
  return (
    <Card data-export-key-file>
      <CardHeader>
        <CardTitle>Export a key file (optional)</CardTitle>
        <CardDescription>
          A Mina encrypted key file, the format a native node reads, under a
          password you choose.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className="flex flex-col gap-3"
          onSubmit={async event => {
            event.preventDefault();
            if (password !== again) {
              setError("The two passwords differ.");
              return;
            }
            setBusy(true);
            setDone(false);
            try {
              const text = await exportKeyFile(glue, heldKey, password);
              download(`mina-key-${heldKey.publicKey.slice(-8)}`, text);
              setError(null);
              setDone(true);
              setPassword("");
              setAgain("");
            } catch (e) {
              setError(e instanceof Error ? e.message : String(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="export-password">Password</Label>
              <Input
                id="export-password"
                type="password"
                autoComplete="new-password"
                value={password}
                onChange={e => setPassword(e.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="export-again">The same again</Label>
              <Input
                id="export-again"
                type="password"
                autoComplete="new-password"
                value={again}
                onChange={e => setAgain(e.target.value)}
              />
            </div>
          </div>
          <p className="text-sm text-muted-foreground">
            At least {MIN_PASSPHRASE} characters.
          </p>
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          {done ? (
            <p className="text-sm text-muted-foreground" data-export-done>
              Downloaded. Run <code>chmod 600</code> on it before a native node
              reads it.
            </p>
          ) : null}
          <Button
            type="submit"
            variant="outline"
            className="w-fit"
            disabled={busy || problem !== null || !again}
          >
            {busy ? "Encrypting…" : "Download key file"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function download(name: string, text: string) {
  const url = URL.createObjectURL(
    new Blob([text], { type: "application/octet-stream" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
