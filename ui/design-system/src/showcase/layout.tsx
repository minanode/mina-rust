import * as React from "react";

import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { cn } from "@minanode/design-system/lib/utils";

const ThemePanelContext = React.createContext<"dark" | "light" | null>(null);

/** The theme of the enclosing ThemePair panel, or null outside one. */
export function useThemePanel() {
  return React.useContext(ThemePanelContext);
}

/** Appends the panel's theme to a label, so regions rendered once per theme
 * keep unique accessible names. */
export function useThemedLabel(label: string) {
  const theme = useThemePanel();
  return theme ? `${label} (${theme})` : label;
}

export function ShowcaseSection({
  id,
  title,
  description,
  children,
}: {
  id: string;
  title: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section
      id={id}
      aria-labelledby={`${id}-title`}
      className="flex flex-col gap-8 border-t py-14"
    >
      <div className="flex max-w-prose flex-col gap-2">
        <h2 id={`${id}-title`} className="text-title text-foreground">
          {title}
        </h2>
        {description ? (
          <p className="leading-relaxed text-pretty text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {children}
    </section>
  );
}

export function Subsection({
  title,
  description,
  children,
  className,
}: {
  title: string;
  description?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col gap-4", className)}>
      <div className="flex flex-col gap-1">
        <h3 className="text-heading text-foreground">{title}</h3>
        {description ? (
          <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {children}
    </div>
  );
}

/**
 * Renders the same content in the dark and the light theme. `split` puts the
 * two side by side from lg up; `stack` keeps them full width.
 */
export function ThemePair({
  children,
  layout = "split",
  className,
}: {
  children: React.ReactNode;
  layout?: "split" | "stack";
  className?: string;
}) {
  return (
    <div
      className={cn(
        "grid gap-4",
        layout === "split" && "lg:grid-cols-2",
        className,
      )}
    >
      {(["dark", "light"] as const).map(theme => (
        <div
          key={theme}
          data-theme-panel={theme}
          className={cn(
            theme,
            "flex min-w-0 flex-col gap-4 rounded-xl border bg-background p-5 text-foreground",
          )}
        >
          <p className="font-mono text-label text-muted-foreground uppercase">
            {theme}
          </p>
          <ThemePanelContext.Provider value={theme}>
            {children}
          </ThemePanelContext.Provider>
        </div>
      ))}
    </div>
  );
}

/** A labelled row of specimens. */
export function Specimen({
  label,
  children,
  className,
}: {
  label: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className="flex flex-col gap-2">
      <p className="font-mono text-xs text-muted-foreground">{label}</p>
      <div className={cn("flex flex-wrap items-center gap-3", className)}>
        {children}
      </div>
    </div>
  );
}

/** Horizontally scrollable, keyboard-focusable wrapper for wide grids. */
export function Scroller({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  const name = useThemedLabel(label);
  return (
    <div
      role="region"
      aria-label={name}
      tabIndex={0}
      className="-mx-1 overflow-x-auto rounded-md px-1 pb-1"
    >
      {children}
    </div>
  );
}

export function CodeBlock({ title, code }: { title: string; code: string }) {
  return (
    <figure className="flex min-w-0 flex-col overflow-hidden rounded-xl border bg-card">
      <figcaption className="flex items-center justify-between gap-3 border-b px-4 py-2.5 font-mono text-xs text-muted-foreground">
        <span>{title}</span>
        <CopyButton text={code} label={title} size="md" />
      </figcaption>
      <pre
        tabIndex={0}
        role="region"
        aria-label={title}
        className="overflow-x-auto whitespace-pre-wrap [overflow-wrap:anywhere] px-4 py-4 font-mono text-[0.8125rem] leading-relaxed text-foreground"
      >
        <code>{code}</code>
      </pre>
    </figure>
  );
}
