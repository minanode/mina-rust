import * as React from "react";
import { Check } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * A native checkbox drawn in the theme: a 16 px box with the field's
 * border, teal with a check when ticked. Pair it with a Label set at
 * `leading-5`, so the box sits centred on the label's first line.
 */
function Checkbox({
  className,
  ...props
}: Omit<React.ComponentProps<"input">, "type">) {
  return (
    <span className="relative inline-flex size-4 shrink-0 translate-y-0.5">
      <input
        type="checkbox"
        data-slot="checkbox"
        className={cn(
          "peer size-4 shrink-0 appearance-none rounded-[4px] border border-field bg-background/60 transition-colors duration-150 checked:border-primary checked:bg-primary focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50",
          className,
        )}
        {...props}
      />
      <Check
        aria-hidden
        strokeWidth={3}
        className="pointer-events-none absolute inset-0 m-auto size-3 text-primary-foreground opacity-0 peer-checked:opacity-100"
      />
    </span>
  );
}

export { Checkbox };
