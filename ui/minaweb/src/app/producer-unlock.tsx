/** At a block producer's restart, where the browser couldn't carry its key
 * by itself (roles/handoff.ts): the passphrase that decrypts it, or a start
 * without producing. */

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
  const [missing, setMissing] = React.useState(false);
  const unlock = boot.unlock;
  if (!unlock) return null;
  return (
    <Card className="gap-4 p-4" data-unlock>
      <div className="flex items-center gap-2">
        <KeyRound aria-hidden className="size-4 text-brand-text" />
        <h2 className="text-base font-semibold text-foreground">
          Unlock the producer's key
        </h2>
      </div>
      <p className="text-sm text-pretty text-muted-foreground">
        Type the passphrase you set before the restart.
      </p>
      <Hash value={unlock.publicKey} label="producer public key" />
      <form
        className="flex flex-col gap-3"
        onSubmit={event => {
          event.preventDefault();
          // The button stays the page's teal action while the field is
          // empty: an empty submit says what is missing instead.
          if (!passphrase) {
            setMissing(true);
            return;
          }
          setMissing(false);
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
            onChange={event => {
              setPassphrase(event.target.value);
              setMissing(false);
            }}
            aria-invalid={unlock.error || missing ? true : undefined}
            aria-describedby={
              unlock.error || missing ? "unlock-error" : undefined
            }
            autoFocus
          />
          {unlock.error || missing ? (
            <p id="unlock-error" className="text-sm text-destructive">
              {missing ? "Type the passphrase." : unlock.error}
            </p>
          ) : null}
        </div>
        {/* On a phone both at the full width, as the page's buttons below
            the card are. */}
        <div className="flex flex-col gap-2 sm:flex-row">
          <Button
            type="submit"
            disabled={unlock.busy}
            className="max-sm:w-full"
          >
            {unlock.busy ? "Unlocking…" : "Unlock and produce"}
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={onSkip}
            className="max-sm:w-full"
          >
            Start without producing
          </Button>
        </div>
      </form>
    </Card>
  );
}
