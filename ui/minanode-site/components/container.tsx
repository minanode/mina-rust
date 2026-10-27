import { cn } from "@minanode/design-system/lib/utils";

export function Container({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      className={cn("mx-auto w-full max-w-page px-gutter", className)}
      {...props}
    />
  );
}
