import * as React from "react";
import { Slot } from "radix-ui";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@minanode/design-system/lib/utils";

const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center justify-center gap-1.5 overflow-hidden rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-[color,background-color,border-color] duration-150 focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring [&>svg]:pointer-events-none [&>svg]:size-3",
  {
    variants: {
      variant: {
        default:
          "border-brand-border bg-brand-subtle text-brand-text [a&]:hover:bg-brand-subtle/70",
        solid:
          "border-transparent bg-primary text-primary-foreground [a&]:hover:bg-teal-400",
        secondary:
          "border-transparent bg-secondary text-secondary-foreground [a&]:hover:bg-accent",
        outline:
          "border-border text-muted-foreground [a&]:hover:border-brand-border [a&]:hover:text-foreground",
        destructive:
          "border-transparent bg-destructive text-destructive-foreground [a&]:hover:bg-destructive/90",
      },
      font: {
        sans: "",
        mono: "font-mono tracking-tight",
      },
    },
    defaultVariants: {
      variant: "default",
      font: "mono",
    },
  },
);

function Badge({
  className,
  variant,
  font,
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span";

  return (
    <Comp
      data-slot="badge"
      className={cn(badgeVariants({ variant, font }), className)}
      {...props}
    />
  );
}

export { Badge, badgeVariants };
