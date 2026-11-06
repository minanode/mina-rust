/** After a role change in a browser that couldn't carry the key by itself
 * (roles/handoff.ts): the key that crossed the restart encrypted with a
 * passphrase, in this tab's sessionStorage, waiting for it. */

import * as React from "react";
import { KeyRound } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Hash } from "@minanode/design-system/patterns/hash";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";

import { useKeyGlue, useKeys } from "../roles/hooks";
import {
  decryptKey,
  HANDOFF_KEY,
  keyStore,
  readStored,
  removeStored,
} from "../roles/keys";

export function CarriedKey() {
  const { key } = useKeys();
  const glue = useKeyGlue();
  // Bumped after a change here, to read the record again.
  const [, setVersion] = React.useState(0);
  const record = readStored("session", HANDOFF_KEY);
  const [passphrase, setPassphrase] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [confirmForget, setConfirmForget] = React.useState(false);
  if (!record || key?.publicKey === record.publicKey) return null;
  return (
    <Card className="mb-6 gap-4 p-4" data-carried-key>
      <div className="flex items-center gap-2">
        <KeyRound aria-hidden className="size-4 text-brand-text" />
        <h2 className="text-base font-semibold text-foreground">
          Unlock the key from before the restart
        </h2>
      </div>
      <p className="text-sm text-pretty text-muted-foreground">
        It crossed the restart encrypted, in this tab only. Type its passphrase
        to hold it again.
      </p>
      <Hash value={record.publicKey} label="carried public key" />
      <form
        className="flex flex-col gap-3"
        onSubmit={async event => {
          event.preventDefault();
          setBusy(true);
          try {
            const held = await decryptKey(
              record,
              passphrase,
              glue ?? undefined,
            );
            keyStore.hold(held);
            removeStored("session", HANDOFF_KEY);
            setError(null);
            setVersion(v => v + 1);
          } catch (e) {
            setError(e instanceof Error ? e.message : "Wrong passphrase.");
          } finally {
            setBusy(false);
            setPassphrase("");
          }
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="carried-passphrase">Passphrase</Label>
          <Input
            id="carried-passphrase"
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
            onClick={() => setConfirmForget(true)}
          >
            Forget it
          </Button>
        </div>
      </form>
      {confirmForget ? (
        <InlineConfirm
          label="Confirm forgetting the carried key"
          message="This deletes the encrypted copy from this tab. Without a backup, the key and anything it holds are lost."
          confirmLabel="Forget it"
          onConfirm={() => {
            removeStored("session", HANDOFF_KEY);
            setConfirmForget(false);
            setVersion(v => v + 1);
          }}
          onCancel={() => setConfirmForget(false)}
        />
      ) : null}
    </Card>
  );
}
