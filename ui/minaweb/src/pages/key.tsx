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
import { Checkbox } from "@minanode/design-system/components/checkbox";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Textarea } from "@minanode/design-system/components/textarea";
import { Amount } from "@minanode/design-system/patterns/amount";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
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
  // A form page: the forms keep to a measure; the header runs to the
  // content edge, as on every page.
  return (
    <div className="flex flex-col gap-6 lg:gap-8">
      <PageHeader
        eyebrow="Devnet"
        title="Your key"
        aside={
          <ExternalLink href={KEY_DOCS_URL} className="text-sm">
            Web node docs
          </ExternalLink>
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
    </div>
  );
}

function NoKey({ glue, hasSaved }: { glue: KeyGlue; hasSaved: boolean }) {
  const [error, setError] = React.useState<string | null>(null);
  const [importing, setImporting] = React.useState(false);
  return (
    <>
      {/* A saved key is the likely task: its unlock comes first, and the
          other ways to a key are outline under it. */}
      {hasSaved ? <UnlockSaved glue={glue} /> : null}
      {/* The same panel Send and the roles show without a key. */}
      <EmptyState
        icon={<KeyRound />}
        title={hasSaved ? "Use another key" : "No key in this tab"}
        description="Back up a new key before you close the tab, or it is gone."
        action={
          <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row sm:flex-wrap">
            {/* Outline while the import or the saved key is the task. */}
            <Button
              type="button"
              variant={importing || hasSaved ? "outline" : "default"}
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
              <Sparkles aria-hidden />
              Generate a new key
            </Button>
            {!importing ? (
              <Button
                type="button"
                variant="outline"
                onClick={() => setImporting(true)}
              >
                Import a devnet key instead
              </Button>
            ) : null}
          </div>
        }
      />
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
      {importing ? (
        <ImportKey glue={glue} onClose={() => setImporting(false)} />
      ) : null}
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
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <CardDescription>
          Saved encrypted with your passphrase. Public key:{" "}
          <Hash value={saved.publicKey} label="saved public key" />
        </CardDescription>
        <form
          className="flex flex-col gap-3"
          onSubmit={async event => {
            event.preventDefault();
            // Unlock stays the teal action while the field is empty: an
            // empty submit says what is missing.
            if (!passphrase) {
              setError("Type the passphrase.");
              return;
            }
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
              // The page's task when a key is saved, as the producer's
              // unlock focuses its field.
              autoFocus
              value={passphrase}
              onChange={e => {
                setPassphrase(e.target.value);
                setError(null);
              }}
              aria-invalid={error ? true : undefined}
            />
            {error ? <p className="text-sm text-destructive">{error}</p> : null}
          </div>
          <div className="flex flex-wrap gap-2">
            {/* On a phone the full width: the page's action outweighs the
                Delete under it. */}
            <Button type="submit" disabled={busy} className="max-sm:w-full">
              {busy ? "Unlocking…" : "Unlock"}
            </Button>
            {/* A key loss, marked as Forget is, and kept apart from
                Unlock: at the card's right edge, on a phone a line of its
                own at its own width, never wider than Unlock's row. */}
            <span aria-hidden className="basis-full sm:hidden" />
            <Button
              type="button"
              variant="destructive-outline"
              className="sm:ml-auto"
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

function ImportKey({ glue, onClose }: { glue: KeyGlue; onClose: () => void }) {
  const fileGlue = useKeyFileGlue();
  const [understood, setUnderstood] = React.useState(false);
  const [kind, setKind] = React.useState<"raw" | "file">("raw");
  const close = onClose;
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
              A key controls the same account on every network, mainnet
              included. Import only a key made for devnet.
            </p>
          </AlertDescription>
        </Alert>
        <label className="flex items-start gap-2 text-sm leading-5 text-foreground">
          <Checkbox
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
          className="text-muted-foreground hover:text-foreground"
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
          className="text-muted-foreground hover:text-foreground"
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
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <CardDescription>
            {key.origin === "generated" ? null : "Imported into this tab. "}
            Share it to receive devnet MINA.
          </CardDescription>
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
            ? [{ label: "Balance", value: "Looking it up…", prose: true }]
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
                    prose: a.delegate === a.publicKey,
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
                    prose: true,
                  },
                ]
        }
        footer={
          a ? (
            "At this node's best tip."
          ) : (
            <>
              The account exists once it receives MINA.{" "}
              {/* The link ends the line, as on Send. */}
              <ExternalLink href={FAUCET_URL}>
                Get devnet MINA from the faucet
              </ExternalLink>
            </>
          )
        }
      />

      <Backup
        privateKey={key.privateKey}
        generated={key.origin === "generated"}
        savedHere={saved?.publicKey === key.publicKey}
      />
      <SaveEncrypted savedHere={saved?.publicKey === key.publicKey} />
      {fileGlue ? <ExportKeyFile glue={fileGlue} heldKey={key} /> : null}

      <div className="flex flex-col gap-3">
        {/* The confirm takes the button's place: one destructive control
            on screen, as on the producer page. */}
        {!confirmForget ? (
          <div>
            <Button
              type="button"
              variant="destructive-outline"
              onClick={() => setConfirmForget(true)}
            >
              Forget the key in this tab
            </Button>
          </div>
        ) : null}
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
  savedHere,
}: {
  privateKey: string;
  generated: boolean;
  savedHere: boolean;
}) {
  const [shown, setShown] = React.useState(false);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Private key</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <CardDescription>
          {!generated
            ? "Anyone with it controls the account, on devnet and on mainnet."
            : savedHere
              ? "This tab's copy goes when the tab closes; the encrypted copy stays in this browser."
              : "Lost when this tab closes, unless copied or saved below."}
        </CardDescription>
        {/* The copy follows its value, as on the public key's line; the
            row below holds only Show or Hide. */}
        {shown ? (
          <div className="relative">
            <code
              className="block rounded-md border bg-muted py-2 pr-12 pl-3 font-mono text-sm wrap-anywhere"
              data-private-key-shown
            >
              {privateKey}
            </code>
            <CopyButton
              text={privateKey}
              label="private key"
              size="md"
              className="absolute top-1 right-1"
            />
          </div>
        ) : (
          <p className="flex items-center gap-1 font-mono text-sm text-muted-foreground">
            EK••••••••
            <CopyButton text={privateKey} label="private key" />
          </p>
        )}
        <div>
          <Button
            type="button"
            variant="outline"
            onClick={() => setShown(s => !s)}
            data-action="reveal-key"
          >
            {shown ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
            {shown ? "Hide it" : "Show it"}
          </Button>
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
          {/* The private key's line says the copy stays. */}
          <p>The passphrase unlocks it in a new tab.</p>
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
              variant="destructive-outline"
              // On the brand tint the red text drops under AA in light: the
              // button keeps the page's colour behind it, and shows hover
              // by its border, not a translucent fill over the tint.
              className="mt-2 w-fit bg-background hover:border-destructive hover:bg-background active:bg-background"
              onClick={() => setConfirmDelete(true)}
            >
              Delete the saved key
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
        <CardTitle>
          Save it in this browser{" "}
          <span className="font-normal text-muted-foreground">optional</span>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <CardDescription>
          Anyone with this device and the passphrase can use it.
        </CardDescription>
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
          <div className="grid gap-4 sm:grid-cols-2">
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
              <Label htmlFor="save-again">Confirm passphrase</Label>
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
        <CardTitle>
          Export a key file{" "}
          <span className="font-normal text-muted-foreground">optional</span>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <CardDescription>Encrypted key file for a native node.</CardDescription>
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
          <div className="grid gap-4 sm:grid-cols-2">
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
              <Label htmlFor="export-again">Confirm password</Label>
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
