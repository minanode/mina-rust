"use client";

import { Clock, RefreshCw } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { formatAge, useNow } from "@minanode/design-system/lib/time";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * How old the figures on a page are, with a refresh button: "Updated 3 s
 * ago"; in amber "Data from 2 min ago" once the source stops answering or
 * the data is older than `staleAfterMs`; "Paused, data from …" when polling
 * is paused on purpose.
 */
function DataAge({
  updatedAt,
  loading = false,
  failed = false,
  staleAfterMs,
  paused = false,
  onRefresh,
  className,
}: {
  /** When the data arrived, ms since the epoch; null before any. */
  updatedAt: number | null;
  /** The first answer is on its way. */
  loading?: boolean;
  /** The latest request failed. */
  failed?: boolean;
  staleAfterMs: number;
  paused?: boolean;
  onRefresh?: () => void;
  className?: string;
}) {
  const now = useNow();
  const age = updatedAt === null ? null : now - updatedAt;
  const stale = !paused && (failed || (age !== null && age > staleAfterMs));
  return (
    <div
      data-slot="data-age"
      className={cn("flex items-center gap-1", className)}
    >
      <p
        className={cn(
          "inline-flex items-center gap-1.5 font-mono text-xs",
          stale && age !== null
            ? "text-status-pending"
            : "text-muted-foreground",
        )}
      >
        <Clock aria-hidden className="size-3.5" />
        {age === null
          ? loading
            ? "Loading…"
            : "No data yet"
          : paused
            ? `Paused, data from ${formatAge(age)} ago`
            : stale
              ? `Data from ${formatAge(age)} ago`
              : `Updated ${formatAge(age)} ago`}
      </p>
      {onRefresh ? (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Refresh"
          title="Refresh"
          onClick={onRefresh}
          className="text-muted-foreground hover:text-foreground"
        >
          <RefreshCw aria-hidden />
        </Button>
      ) : null}
    </div>
  );
}

export { DataAge };
