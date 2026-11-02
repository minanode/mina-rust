/** At a block producer's restart: the passphrase that decrypts its key, or
 * a start without producing. */

import * as React from "react";
import { KeyRound } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Card } from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Hash } from "@minanode/design-system/patterns/hash";

import type { BootSnapshot } from "../boot/boot";

export function ProducerUnlock({
  boot,
  onUnlock,
  onSkip,
}: {
  boot: BootSnapshot;
  onUnlock: (passphrase: string) => void;
  onSkip: () => void;
}) {
  const [passphrase, setPassphrase] = React.useState("");
  const unlock = boot.unlock;
  if (!unlock) return null;
  return (
    <Card className="gap-4 p-6" data-unlock>
      <div className="flex items-center gap-2">
        <KeyRound aria-hidden className="size-4 text-brand-text" />
        <h2 className="text-base font-semibold text-foreground">
          Unlock the producer's key
        </h2>
      </div>
      <p className="text-sm text-pretty text-muted-foreground">
        The node starts again as the block producer for this key. Its private
        key crossed the restart encrypted with your passphrase, in this tab
        only. Type the passphrase to start producing.
      </p>
      <Hash value={unlock.publicKey} label="producer public key" />
      <form
        className="flex flex-col gap-3"
        onSubmit={event => {
          event.preventDefault();
          onUnlock(passphrase);
          setPassphrase("");
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="unlock-passphrase">Passphrase</Label>
          <Input
            id="unlock-passphrase"
            type="password"
            autoComplete="current-password"
            value={passphrase}
            onChange={event => setPassphrase(event.target.value)}
            aria-invalid={unlock.error ? true : undefined}
            aria-describedby={unlock.error ? "unlock-error" : undefined}
            autoFocus
          />
          {unlock.error ? (
            <p id="unlock-error" className="text-sm text-destructive">
              {unlock.error}
            </p>
          ) : null}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={unlock.busy || !passphrase}>
            {unlock.busy ? "Unlocking…" : "Unlock and produce"}
          </Button>
          <Button type="button" variant="outline" onClick={onSkip}>
            Start without producing
          </Button>
        </div>
      </form>
    </Card>
  );
}
