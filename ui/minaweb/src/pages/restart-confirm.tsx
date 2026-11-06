/**
 * The confirmation before a role change, which starts the node again by
 * reloading the page. The tab's key crosses the reload by itself
 * (roles/handoff.ts). Where the browser can't do that, the first attempt
 * says so and nothing restarts; then a key that isn't saved in this
 * browser would be lost, so the visitor carries it across encrypted with a
 * passphrase, or says plainly to restart without it. The confirm button
 * waits for one or the other.
 */

import * as React from "react";
import { TriangleAlert } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { confirmTint } from "@minanode/design-system/patterns/inline-confirm";
import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

import { HandoffError, handoffSupported } from "../roles/handoff";
import { MIN_PASSPHRASE, passphraseProblem } from "../roles/keys";
import { useKeys } from "../roles/hooks";
import { keyAtRisk, restartWithKey, type KeyChoice } from "../roles/restart";
import type { Role } from "../roles/role";

export function RestartConfirm({
  label,
  message,
  confirmLabel,
  role,
  tone = "caution",
  className,
  onCancel,
  restart = restartWithKey,
}: {
  label: string;
  message: React.ReactNode;
  confirmLabel: string;
  /** `caution` for a restart; `destructive` when it stops a running role,
   * which loses its work. */
  tone?: "caution" | "destructive";
  className?: string;
  /** The role the node starts with. */
  role: Role;
  onCancel: () => void;
  /** For tests. */
  restart?: typeof restartWithKey;
}) {
  const { key, saved } = useKeys();
  // Set once the key can't cross the reload by itself.
  const [fallback, setFallback] = React.useState(() => !handoffSupported());
  const atRisk = fallback && keyAtRisk(key, saved);
  const [mode, setMode] = React.useState<"carry" | "drop">("carry");
  const [passphrase, setPassphrase] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  const choice: KeyChoice | null =
    !fallback || !key
      ? null
      : !atRisk || mode === "drop"
        ? { kind: "drop" }
        : passphraseProblem(passphrase)
          ? null
          : { kind: "carry", passphrase };
  const ready = !atRisk || choice !== null;

  return (
    <div
      role="group"
      aria-label={label}
      data-restart-confirm
      data-tone={tone}
      className={cn(
        "flex flex-col gap-3 rounded-lg border p-4",
        confirmTint[tone],
        // Inside a card, a tint without a border, as InlineConfirm.
        borderlessInSurface,
        className,
      )}
    >
      <div className="max-w-prose text-sm text-foreground">{message}</div>
      {atRisk ? (
        <div className="flex flex-col gap-3" data-key-at-risk>
          <p className="flex gap-2 text-sm text-foreground">
            <TriangleAlert
              aria-hidden
              className="mt-0.5 size-4 shrink-0 text-status-pending"
            />
            <span>
              The restart empties this tab's memory, and this tab's key isn't
              saved in this browser. Keep it through the restart, or it is gone.
            </span>
          </p>
          <label className="flex items-start gap-2 text-sm text-foreground">
            <input
              type="radio"
              name="restart-key"
              className="mt-0.5 size-4 shrink-0 accent-primary"
              checked={mode === "carry"}
              onChange={() => setMode("carry")}
            />
            Keep the key: encrypt it with a passphrase, in this tab only, and
            unlock it after the restart.
          </label>
          {mode === "carry" ? (
            <div className="flex flex-col gap-1.5 pl-6">
              <Label htmlFor="restart-passphrase">Passphrase</Label>
              <Input
                id="restart-passphrase"
                type="password"
                autoComplete="new-password"
                value={passphrase}
                onChange={e => setPassphrase(e.target.value)}
              />
              {/* On the caution tint: the muted text a step darker, for AA. */}
              <p className="text-sm text-muted-on-tint">
                At least {MIN_PASSPHRASE} characters.
              </p>
            </div>
          ) : null}
          <label className="flex items-start gap-2 text-sm text-foreground">
            <input
              type="radio"
              name="restart-key"
              className="mt-0.5 size-4 shrink-0 accent-primary"
              checked={mode === "drop"}
              onChange={() => setMode("drop")}
              data-action="drop-key"
            />
            Restart without it. I have a backup of the private key, or I don't
            need it: without one, the key and anything it holds are lost.
          </label>
        </div>
      ) : fallback && key ? (
        <p className="text-sm text-muted-on-tint">
          This tab's key is saved in this browser: unlock it on Your key after
          the restart.
        </p>
      ) : null}
      {error ? (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant={tone === "destructive" ? "destructive" : "default"}
          disabled={!ready || busy}
          aria-busy={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            try {
              await restart(role, key, choice);
            } catch (e) {
              if (e instanceof HandoffError) setFallback(true);
              setError(e instanceof Error ? e.message : String(e));
              setBusy(false);
            }
          }}
        >
          {busy ? "Restarting…" : confirmLabel}
        </Button>
        {/* Quieter than the confirm by its weight, not its colour: muted
            text drops under AA on the caution tint in light. */}
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={onCancel}
        >
          Cancel
        </Button>
      </div>
    </div>
  );
}
