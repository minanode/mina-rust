import * as React from "react";

import { borderlessInSurface } from "@minanode/design-system/lib/surface";
import { cn } from "@minanode/design-system/lib/utils";
import { Stat } from "@minanode/design-system/patterns/stat";

type StatRowItem = {
  label: React.ReactNode;
  value: React.ReactNode;
  detail?: React.ReactNode;
};

/**
 * A row of labelled figures with hairlines between them, as a description
 * list: a summary such as a period's counts and times. It wraps into as
 * many columns as fit, each at least `minItemWidth` wide. `label` names it
 * as a group, to tell rows apart. A box of its own; inside a Card or a
 * KeyValueList it drops its border (lib/surface).
 */
function StatRow({
  items,
  label,
  minItemWidth = "9.5rem",
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "children"> & {
  items: StatRowItem[];
  label?: string;
  minItemWidth?: string;
}) {
  return (
    <div
      data-slot="stat-row"
      role={label ? "group" : undefined}
      aria-label={label}
      className={cn(
        "overflow-hidden rounded-xl border bg-card",
        borderlessInSurface,
        className,
      )}
      {...props}
    >
      {/* Each figure draws its top and left hairline; the row's edge hides
          those of the first row and column. */}
      <dl
        className="-mt-px -ml-px grid"
        style={{
          gridTemplateColumns: `repeat(auto-fit, minmax(min(100%, ${minItemWidth}), 1fr))`,
        }}
      >
        {items.map((item, i) => (
          <Stat
            key={i}
            variant="labelled"
            inList
            size="sm"
            className="border-t border-l px-4 py-3.5"
            label={item.label}
            value={item.value}
            detail={item.detail}
          />
        ))}
      </dl>
    </div>
  );
}

export { StatRow, type StatRowItem };
