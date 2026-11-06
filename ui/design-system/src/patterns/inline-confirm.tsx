import * as React from "react";

import { Button } from "@minanode/design-system/components/button";
import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * A confirmation that opens in place, for an action that can't be undone
 * from the page: what will happen, a confirm and a way back. `tone`:
 * `destructive` (the default) for a loss, such as stopping a worker or
 * deleting a key, in red; `caution` for a change with a cost but no loss,
 * such as a restart, in amber with a primary confirm. Render it when the
 * user asks for the action; it isn't a modal, so the page stays readable
 * around it. Inside a Card it keeps its tint but no border (`lib/surface`).
 */
/** A confirmation's panel tint, by tone. */
const confirmTint = {
  // Inside a card, a stronger tint: the 8% one reads grey there.
  destructive:
    "border-destructive/40 bg-destructive/8 in-data-[slot=card]:bg-destructive/12",
  caution:
    "border-status-pending/40 bg-caution-wash in-data-[slot=card]:bg-caution-wash-strong",
} as const;

function InlineConfirm({
  label,
  message,
  confirmLabel,
  busyLabel,
  cancelLabel = "Cancel",
  busy = false,
  tone = "destructive",
  onConfirm,
  onCancel,
}: {
  /** The group's accessible name: "Confirm stopping the worker". */
  label: string;
  message: React.ReactNode;
  confirmLabel: string;
  /** Shown on the confirm button while `busy`. */
  busyLabel?: string;
  cancelLabel?: string;
  busy?: boolean;
  tone?: "destructive" | "caution";
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      data-slot="inline-confirm"
      role="group"
      aria-label={label}
      data-tone={tone}
      className={cn(
        "flex flex-col gap-3 rounded-lg border p-4",
        confirmTint[tone],
        borderlessInSurface,
      )}
    >
      <p className="max-w-prose text-sm text-foreground">{message}</p>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant={tone === "destructive" ? "destructive" : "default"}
          disabled={busy}
          aria-busy={busy}
          onClick={onConfirm}
        >
          {busy && busyLabel ? busyLabel : confirmLabel}
        </Button>
        {/* Quieter than the confirm by its weight, not its colour: muted
            text drops under AA on the tint in light. */}
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={onCancel}
        >
          {cancelLabel}
        </Button>
      </div>
    </div>
  );
}

export { confirmTint, InlineConfirm };
