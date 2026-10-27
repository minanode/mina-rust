"use client";

import { formatAge, useNow } from "@minanode/design-system/lib/time";
import { cn } from "@minanode/design-system/lib/utils";

const missing = <span className="text-muted-foreground">–</span>;

/** "45 s ago", counting, with the UTC time in its title. */
function TimeAgo({ at, className }: { at: number | null; className?: string }) {
  const now = useNow();
  if (at === null) return missing;
  return (
    <time
      dateTime={new Date(at).toISOString()}
      title={new Date(at).toISOString().replace("T", " ").slice(0, 19) + " UTC"}
      className={cn("whitespace-nowrap tabular-nums", className)}
    >
      {formatAge(now - at)} ago
    </time>
  );
}

/** "in 2 h 5 min", counting down; "now" once it's past. */
function TimeUntil({
  at,
  className,
}: {
  at: number | null;
  className?: string;
}) {
  const now = useNow();
  if (at === null) return missing;
  return (
    <time
      dateTime={new Date(at).toISOString()}
      className={cn("whitespace-nowrap tabular-nums", className)}
    >
      {at > now ? `in ${formatAge(at - now)}` : "now"}
    </time>
  );
}

/** "45 s ago" for a past time, "in 25 min" for a future one: for times
 * that can be either side of now (a won slot), where TimeAgo would clamp a
 * future time to "0 s ago". The UTC time is in its title. */
function RelativeTime({
  at,
  className,
}: {
  at: number | null;
  className?: string;
}) {
  const now = useNow();
  if (at === null) return missing;
  return (
    <time
      dateTime={new Date(at).toISOString()}
      title={new Date(at).toISOString().replace("T", " ").slice(0, 19) + " UTC"}
      className={cn("whitespace-nowrap tabular-nums", className)}
    >
      {at > now ? `in ${formatAge(at - now)}` : `${formatAge(now - at)} ago`}
    </time>
  );
}

export { RelativeTime, TimeAgo, TimeUntil };
