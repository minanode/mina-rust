import * as React from "react";

import { useMoreToTheRight } from "@minanode/design-system/lib/more-to-the-right";
import { cn } from "@minanode/design-system/lib/utils";

function Table({
  className,
  containerProps,
  density = "default",
  ...props
}: React.ComponentProps<"table"> & {
  /** Props for the scroll container. When the table can overflow, give it a
   * `tabIndex={0}`, `role="region"` and an accessible name so keyboard users
   * can scroll it. */
  containerProps?: React.ComponentProps<"div">;
  /** `compact`: shorter rows, for long lists of one-line records. */
  density?: "default" | "compact";
}) {
  const more = useMoreToTheRight<HTMLDivElement>();
  return (
    <div
      data-slot="table-container"
      {...containerProps}
      ref={more.ref}
      data-more-right={more.right || undefined}
      className={cn(
        // A table wider than its box fades at the right edge while columns
        // are hidden there, so a sideways scroll is discoverable.
        "relative w-full overflow-x-auto rounded-xl border bg-card data-more-right:[mask-image:linear-gradient(to_right,black_calc(100%-2.5rem),transparent)]",
        containerProps?.className,
      )}
    >
      <table
        data-slot="table"
        data-density={density}
        className={cn(
          // Machine values at one mono size and the text's line height, so
          // a row's baselines match.
          "w-full caption-bottom text-sm [&_tbody_.font-mono]:text-[0.8125rem] [&_tbody_.font-mono]:leading-5",
          // A copy button doesn't make its row taller than the others.
          density === "compact"
            ? "[&_td]:py-2 [&_td_[data-slot=copy-button]]:-my-1 [&_th]:h-9"
            : "[&_td_[data-slot=copy-button]]:-my-0.5",
          className,
        )}
        {...props}
      />
    </div>
  );
}

function TableHeader({ className, ...props }: React.ComponentProps<"thead">) {
  return (
    <thead
      data-slot="table-header"
      className={cn("bg-muted/40 [&_tr]:border-b", className)}
      {...props}
    />
  );
}

function TableBody({ className, ...props }: React.ComponentProps<"tbody">) {
  return (
    <tbody
      data-slot="table-body"
      className={cn("[&_tr:last-child]:border-0", className)}
      {...props}
    />
  );
}

function TableFooter({ className, ...props }: React.ComponentProps<"tfoot">) {
  return (
    <tfoot
      data-slot="table-footer"
      className={cn(
        "border-t bg-muted/50 font-medium [&>tr]:last:border-b-0",
        className,
      )}
      {...props}
    />
  );
}

function TableRow({ className, ...props }: React.ComponentProps<"tr">) {
  return (
    <tr
      data-slot="table-row"
      className={cn(
        "border-b transition-colors duration-150 hover:bg-muted/40 data-[state=selected]:bg-brand-subtle",
        className,
      )}
      {...props}
    />
  );
}

function TableHead({ className, ...props }: React.ComponentProps<"th">) {
  return (
    <th
      data-slot="table-head"
      className={cn(
        "h-10 px-4 text-left align-middle text-xs font-medium whitespace-nowrap text-muted-foreground [&:has([role=checkbox])]:pr-0",
        className,
      )}
      {...props}
    />
  );
}

function TableCell({ className, ...props }: React.ComponentProps<"td">) {
  return (
    <td
      data-slot="table-cell"
      className={cn(
        "px-4 py-3 align-top [&:has([role=checkbox])]:pr-0",
        className,
      )}
      {...props}
    />
  );
}

function TableCaption({
  className,
  ...props
}: React.ComponentProps<"caption">) {
  return (
    <caption
      data-slot="table-caption"
      className={cn(
        "border-t px-4 py-3 text-left text-sm text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** Props for a table's scroll container: a focusable, named region, so
 * keyboard users can scroll it. Its name ends in "table", so it differs from
 * a section around it of the same name. */
function tableRegion(label: string) {
  return {
    tabIndex: 0,
    role: "region",
    "aria-label": `${label} table`,
  } as const;
}

export {
  tableRegion,
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
};
