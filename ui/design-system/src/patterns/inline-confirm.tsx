import * as React from "react";

import { Button } from "@minanode/design-system/components/button";
import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * A confirmation that opens in place, for an action that can't be undone
 * from the page (stopping a worker): what will happen, a destructive confirm
 * and a way back. Render it when the user asks for the action; it isn't a
 * modal, so the page stays readable around it. Inside a Card it keeps its
 * tint but no border (`lib/surface`).
 */
function InlineConfirm({
  label,
  message,
  confirmLabel,
  busyLabel,
  cancelLabel = "Cancel",
  busy = false,
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
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      data-slot="inline-confirm"
      role="group"
      aria-label={label}
      className={cn(
        "flex flex-col gap-3 rounded-lg border border-destructive/40 bg-destructive/8 p-4",
        borderlessInSurface,
      )}
    >
      <p className="text-sm text-foreground">{message}</p>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="destructive"
          size="sm"
          disabled={busy}
          aria-busy={busy}
          onClick={onConfirm}
        >
          {busy && busyLabel ? busyLabel : confirmLabel}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={busy}
          onClick={onCancel}
        >
          {cancelLabel}
        </Button>
      </div>
    </div>
  );
}

export { InlineConfirm };
