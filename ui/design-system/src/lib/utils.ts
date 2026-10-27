import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// tailwind-merge has to know the custom type scale, or it would treat
// `text-label` as a colour and drop it next to `text-muted-foreground`.
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: ["display", "title", "heading", "lead", "metric", "label"],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
