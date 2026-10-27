import * as React from "react";
import { Slot } from "radix-ui";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@minanode/design-system/lib/utils";

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap transition-[color,background-color,border-color,box-shadow] duration-150 ease-standard outline-hidden select-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-glow hover:bg-teal-400 active:bg-teal-600",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-accent active:bg-accent/70",
        outline:
          "border border-field bg-transparent text-foreground hover:border-brand-border hover:bg-brand-subtle active:bg-brand-subtle/60",
        ghost:
          "text-foreground hover:bg-accent hover:text-accent-foreground active:bg-accent/70",
        link: "h-auto px-0 text-brand-text underline-offset-4 hover:underline active:underline",
        destructive:
          "bg-destructive text-destructive-foreground hover:bg-destructive/90 active:bg-destructive/90",
      },
      size: {
        sm: "h-8 gap-1.5 px-3 text-[0.8125rem]",
        default: "h-10 px-4 text-sm",
        lg: "h-12 px-6 text-base",
        icon: "size-10",
        "icon-sm": "size-8",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

function Button({
  className,
  variant,
  size,
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
  }) {
  const Comp = asChild ? Slot.Root : "button";

  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants };
