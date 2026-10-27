import Image from "next/image";

/** A responsive screenshot that can be opened at its original size. */
export function Screenshot({
  src,
  alt,
  caption,
  width,
  height,
  eager = false,
}: {
  src: string;
  alt: string;
  caption: string;
  width: number;
  height: number;
  eager?: boolean;
}) {
  return (
    <figure className="flex min-w-0 flex-col gap-3 xl:w-[calc(100%+12rem)]">
      <a
        href={src}
        target="_blank"
        rel="noopener noreferrer"
        aria-label={`Open full-size image: ${alt}`}
        className="relative block cursor-zoom-in overflow-hidden rounded-xl border bg-card shadow-sm focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        <Image
          src={src}
          alt={alt}
          width={width}
          height={height}
          loading={eager ? "eager" : "lazy"}
          className="h-auto w-full"
        />
        <span className="absolute right-3 bottom-3 rounded-md border bg-card/95 px-2 py-1 text-xs text-foreground">
          Open full size ↗
        </span>
      </a>
      <figcaption className="text-sm leading-relaxed text-muted-foreground">
        {caption}
      </figcaption>
    </figure>
  );
}
