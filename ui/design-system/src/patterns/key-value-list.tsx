import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

type Row = {
  label: React.ReactNode;
  value: React.ReactNode;
  /** Values worth drawing the eye to: brand colour in a receipt, medium
   * weight in a panel, where teal would read as a link. */
  tone?: "default" | "brand";
  /** A value that is words, not a figure: set in the text face. */
  prose?: boolean;
  /** A long value (a full hash, a sentence): under its label, at every
   * width. */
  wide?: boolean;
  /** What qualifies the value ("while proving a block"): a quieter line
   * under the label, so it never wraps ragged at the right. */
  note?: React.ReactNode;
};

/**
 * A titled panel of label and value rows, rendered as a description list.
 *
 * - `variant="panel"` (the apps): labels in the text face, values in mono
 *   with tabular figures, a rule between rows, and label and value on one
 *   row at every width. `wide` rows stack. `align="left"` starts the
 *   values a label column in, for a panel at full width.
 * - `variant="receipt"` (the site): a monospace receipt, every part in
 *   mono; rows stack below 640 px.
 */
function KeyValueList({
  align = "right",
  variant = "panel",
  title,
  aside,
  rows,
  footer,
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "title" | "children"> & {
  align?: "left" | "right";
  variant?: "panel" | "receipt";
  title: React.ReactNode;
  aside?: React.ReactNode;
  rows: Row[];
  footer?: React.ReactNode;
}) {
  const receipt = variant === "receipt";
  return (
    <div
      data-slot="key-value-list"
      className={cn(
        "flex flex-col rounded-xl border bg-card in-[.bg-card]:bg-background/60",
        receipt
          ? align === "left"
            ? "font-sans text-sm"
            : "font-mono text-[0.8125rem]"
          : "text-sm",
        className,
      )}
      {...props}
    >
      <div className="flex min-h-12 flex-wrap items-center justify-between gap-2 border-b px-4 py-2.5">
        <div
          className={cn(
            "min-w-0 text-foreground",
            receipt ? "font-medium" : "text-[0.9375rem] font-semibold",
          )}
        >
          {title}
        </div>
        {aside}
      </div>
      <dl
        className={cn(
          "flex flex-1 flex-col px-4",
          receipt
            ? "gap-2.5 py-4"
            : "divide-y divide-border/70 py-1 [&_[data-slot=copy-button]]:-my-1",
        )}
      >
        {rows.map((row, i) => (
          <div
            key={i}
            className={cn(
              receipt
                ? "grid grid-cols-1 gap-0.5 sm:grid-cols-[minmax(9rem,auto)_1fr] sm:gap-4"
                : row.wide
                  ? "flex flex-col gap-1 py-2.5"
                  : align === "left"
                    ? // A panel at full width: the value starts a label's
                      // width in, not a card's width away.
                      // On a phone the standard row, its value at the right.
                      "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-4 py-2.5 sm:grid-cols-[minmax(9rem,14rem)_minmax(0,1fr)]"
                    : "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-4 py-2.5",
            )}
          >
            <dt className="text-muted-foreground">
              {row.label}
              {row.note ? (
                <span className="block pt-0.5 text-xs text-pretty">
                  {row.note}
                </span>
              ) : null}
            </dt>
            <dd
              className={cn(
                "min-w-0 break-words",
                !receipt &&
                  (row.prose
                    ? "text-pretty"
                    : "font-mono text-[0.8125rem] leading-5 tabular-nums"),
                align === "right" &&
                  (receipt ? "sm:text-right" : !row.wide && "text-right"),
                align === "left" &&
                  !receipt &&
                  !row.wide &&
                  "text-right sm:text-left",
                row.tone !== "brand"
                  ? "text-foreground"
                  : receipt
                    ? "text-brand-text"
                    : "font-medium text-foreground",
              )}
            >
              {row.value}
            </dd>
          </div>
        ))}
      </dl>
      {footer ? (
        <div className="border-t px-4 py-3 font-sans text-sm text-pretty text-muted-foreground">
          {footer}
        </div>
      ) : null}
    </div>
  );
}

export { KeyValueList, type Row as KeyValueRow };
