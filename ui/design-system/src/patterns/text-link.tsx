import * as React from "react";
import { ArrowRight } from "lucide-react";
import { Slot } from "radix-ui";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * An in-app text link in brand teal, underlined on hover, with the focus
 * ring. `asChild` styles the app's own router link instead of an `<a>`.
 * For links that leave the site, use ExternalLink.
 */
function TextLink({
  asChild = false,
  arrow = false,
  className,
  children,
  ...props
}: React.ComponentProps<"a"> & {
  asChild?: boolean;
  /** An arrow after the words, for "go to that page" links. */
  arrow?: boolean;
}) {
  const Comp = asChild ? Slot.Root : "a";
  const icon = arrow ? <ArrowRight aria-hidden className="size-3.5" /> : null;
  return (
    <Comp
      data-slot="text-link"
      className={cn(
        "rounded-sm text-brand-text underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        arrow && "inline-flex w-fit items-center gap-1",
        className,
      )}
      {...props}
    >
      {/* Slot merges onto the one Slottable child: no fragment around it. */}
      {asChild ? <Slot.Slottable>{children}</Slot.Slottable> : children}
      {icon}
    </Comp>
  );
}

export { TextLink };
