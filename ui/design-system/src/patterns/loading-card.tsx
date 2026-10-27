import { Skeleton } from "@minanode/design-system/components/skeleton";
import { cn } from "@minanode/design-system/lib/utils";

/** A card of placeholder rows while the first answer is on its way, with
 * the loading text for screen readers. */
function LoadingCard({
  rows = 3,
  label = "Loading",
  className,
}: {
  rows?: number;
  /** "Loading the peer list": said by screen readers, with an ellipsis. */
  label?: string;
  className?: string;
}) {
  return (
    <div
      data-slot="loading-card"
      role="status"
      aria-live="polite"
      className={cn(
        "flex flex-col gap-3 rounded-xl border bg-card p-6",
        className,
      )}
    >
      <span className="sr-only">{label}…</span>
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className={cn("h-5", i % 2 ? "w-2/3" : "w-full")} />
      ))}
    </div>
  );
}

export { LoadingCard };
