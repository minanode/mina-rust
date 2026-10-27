"use client";

import { cn } from "@minanode/design-system/lib/utils";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";

/**
 * A long identifier (hash, public key, peer id) in monospace: its ends when
 * `truncate`, the whole value in the title and to screen readers, and a
 * button that copies it.
 */
function Hash({
  value,
  truncate = true,
  head = 6,
  tail = 6,
  copy = true,
  label,
  className,
}: {
  value: string;
  truncate?: boolean;
  head?: number;
  tail?: number;
  copy?: boolean;
  /** What the value is, for the copy button's name: "Copy public key". */
  label?: string;
  className?: string;
}) {
  const short =
    truncate && value.length > head + tail + 1
      ? `${value.slice(0, head)}…${tail > 0 ? value.slice(-tail) : ""}`
      : value;
  return (
    <span
      data-slot="hash"
      className={cn(
        "inline-flex max-w-full min-w-0 items-center gap-1 font-mono text-[0.8125rem]",
        className,
      )}
    >
      <span
        title={value}
        className={cn(truncate ? "whitespace-nowrap" : "break-all")}
      >
        {short !== value ? (
          <>
            <span aria-hidden>{short}</span>
            <span className="sr-only">{value}</span>
          </>
        ) : (
          value
        )}
      </span>
      {copy ? <CopyButton text={value} label={label ?? "value"} /> : null}
    </span>
  );
}

export { Hash };
