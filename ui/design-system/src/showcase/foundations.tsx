"use client";

import * as React from "react";
import { Check, X } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";
import { Button } from "@minanode/design-system/components/button";
import { StatusDot } from "@minanode/design-system/patterns/status-badge";
import contrastPairs from "@minanode/design-system/tokens/contrast-pairs.json";

import {
  computed,
  contrast,
  layerExpression,
  paint,
  rawToken,
  toHex,
  type Theme,
} from "./color";
import {
  Scroller,
  ShowcaseSection,
  Specimen,
  Subsection,
  ThemePair,
  useThemePanel,
} from "./layout";

// Names only: values are read from the live CSS, so this page cannot drift
// from tokens.css. A renamed or deleted token shows up here as blank.
const scales = [
  {
    name: "Teal",
    note: "Brand. 500 is the logo colour.",
    steps: ["50", "100", "200", "300", "400", "500", "600", "700", "800", "900", "950"].map(
      s => `--color-teal-${s}`,
    ),
  },
  {
    name: "Ink",
    note: "Neutrals, from page to text.",
    steps: ["50", "100", "200", "300", "400", "500", "600", "700", "800", "850", "900", "950"].map(
      s => `--color-ink-${s}`,
    ),
  },
  {
    name: "Signal",
    note: "Status and charts: a light step for dark surfaces, a dark step for light ones.",
    steps: [
      "--color-sky-400",
      "--color-sky-700",
      "--color-amber-400",
      "--color-amber-700",
      "--color-red-400",
      "--color-red-700",
      "--color-violet-400",
      "--color-violet-700",
      "--color-mint-300",
    ],
  },
];

const semantic: { group: string; tokens: [string, string][] }[] = [
  {
    group: "Surfaces and text",
    tokens: [
      ["--background", "Page background"],
      ["--foreground", "Body text"],
      ["--card", "Cards and panels"],
      ["--card-foreground", "Text on cards"],
      ["--popover", "Menus and popovers"],
      ["--popover-foreground", "Text on popovers"],
      ["--muted", "Quiet surfaces"],
      ["--muted-foreground", "Secondary text"],
      ["--accent", "Hover surface"],
      ["--accent-foreground", "Text on the hover surface"],
      ["--border", "Dividers and card borders"],
      ["--input", "Control borders"],
      ["--field", "Form field borders (3:1)"],
      ["--ring", "Focus ring"],
    ],
  },
  {
    group: "Actions",
    tokens: [
      ["--primary", "Primary action fill"],
      ["--primary-foreground", "Text on primary"],
      ["--secondary", "Secondary action fill"],
      ["--secondary-foreground", "Text on secondary"],
      ["--destructive", "Destructive action fill"],
      ["--destructive-foreground", "Text on destructive"],
    ],
  },
  {
    group: "Brand",
    tokens: [
      ["--brand", "Brand teal fill"],
      ["--brand-foreground", "Text on brand fill"],
      ["--brand-text", "Brand text, links, metric values"],
      ["--brand-subtle", "Tinted brand surface"],
      ["--brand-border", "Brand outline"],
      ["--gradient-from", "Hero gradient start"],
      ["--gradient-to", "Hero gradient end"],
      ["--glow", "Hero glow"],
    ],
  },
  {
    group: "Evidence status",
    tokens: [
      ["--status-live", "Working, on main"],
      ["--status-devnet", "Shown on public devnet"],
      ["--status-pending", "In progress or in preparation"],
      ["--status-planned", "Planned or coming soon"],
    ],
  },
  {
    group: "Charts",
    tokens: [1, 2, 3, 4, 5].map(i => [`--chart-${i}`, `Series ${i}`] as [string, string]),
  },
];

const typeScale = [
  {
    token: "text-display",
    className: "text-display",
    sample: "Client diversity for Mina.",
    note: "Hero headline (h1). 38–72 px fluid, weight 700, tracking −3.5%.",
  },
  {
    token: "text-title",
    className: "text-title",
    sample: "A second, independent Mina client",
    note: "Section headings (h2). 28–40 px fluid, weight 650.",
  },
  {
    token: "text-heading",
    className: "text-heading",
    sample: "Block producer",
    note: "Card and sub-section headings (h3). 20 px, weight 600.",
  },
  {
    token: "text-lead",
    className: "text-lead text-muted-foreground",
    sample: "The same Rust code runs natively or as a full node in a browser tab.",
    note: "Intro paragraphs. 17–20 px fluid, line height 1.6.",
  },
  {
    token: "text-base",
    className: "text-base",
    sample: "Body copy for paragraphs and list items.",
    note: "Body. 16 px, line height 1.5.",
  },
  {
    token: "text-sm",
    className: "text-sm text-muted-foreground",
    sample: "Supporting text, captions and caveats.",
    note: "Supporting text. 14 px.",
  },
  {
    token: "text-metric",
    className: "text-metric font-mono text-brand-text tabular-nums",
    sample: "15,897 / 15,897",
    note: "Metric values. Mono, tabular figures, 28–36 px fluid.",
  },
  {
    token: "text-label",
    className: "text-label font-mono uppercase text-brand-text",
    sample: "Verified proof points",
    note: "Eyebrows, table heads, badges. 12 px mono, uppercase, tracking +6%.",
  },
];

const spacingSteps = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24];

const radii = [
  ["rounded-sm", "--radius-sm"],
  ["rounded-md", "--radius-md"],
  ["rounded-lg", "--radius-lg"],
  ["rounded-xl", "--radius-xl"],
  ["rounded-2xl", "--radius-2xl"],
  ["rounded-full", ""],
];

/** Reads values after mount; the server render shows the structure only. */
function useMounted() {
  const [mounted, setMounted] = React.useState(false);
  React.useEffect(() => setMounted(true), []);
  return mounted;
}

function usePrimitiveNames() {
  const mounted = useMounted();
  return React.useMemo(() => {
    const names = new Map<string, string>();
    if (!mounted) return names;
    for (const scale of scales) {
      for (const token of scale.steps) {
        const value = rawToken("dark", token).toLowerCase();
        if (value) names.set(value, token.replace("--color-", ""));
      }
    }
    names.set("#ffffff", "white");
    names.set("#000000", "black");
    return names;
  }, [mounted]);
}

function Swatch({ token }: { token: string }) {
  const mounted = useMounted();
  const value = mounted ? rawToken("dark", token) : "";
  const name = token.replace("--color-", "");
  return (
    <li className="flex min-w-0 flex-col gap-1.5">
      <span
        aria-hidden
        className="h-14 rounded-lg border border-black/10"
        style={{ background: `var(${token})` }}
      />
      <span className="font-mono text-xs text-foreground">{name}</span>
      <span className="font-mono text-xs text-muted-foreground">
        {value || " "}
      </span>
    </li>
  );
}

function SemanticValue({
  theme,
  token,
  names,
}: {
  theme: Theme;
  token: string;
  names: Map<string, string>;
}) {
  const mounted = useMounted();
  const raw = mounted ? rawToken(theme, token) : "";
  const label = names.get(raw.toLowerCase()) ?? raw.replace(/\s+/g, " ");
  return (
    <div className="flex min-w-0 items-center gap-2.5">
      <span
        aria-hidden
        className={cn(theme, "size-7 shrink-0 rounded-md border border-ink-500/30")}
        style={{
          background: `linear-gradient(var(${token}), var(${token})), ${
            theme === "dark" ? "#0a0f14" : "#f5f8f9"
          }`,
        }}
      />
      <span className="min-w-0 font-mono text-xs break-words text-muted-foreground">
        {label}
      </span>
    </div>
  );
}

function ContrastRow({
  theme,
  pair,
}: {
  theme: Theme;
  pair: (typeof contrastPairs.pairs)[number];
}) {
  const mounted = useMounted();
  let ratio = 0;
  let fg = "";
  let bg = "";
  if (mounted) {
    const layers = pair.bg.map(spec => computed(theme, layerExpression(spec)));
    const bgRgb = paint(layers);
    const fgRgb = paint([...layers, computed(theme, layerExpression(pair.fg))]);
    ratio = contrast(fgRgb, bgRgb);
    fg = toHex(fgRgb);
    bg = toHex(bgRgb);
  }
  const pass = ratio >= pair.min;
  return (
    <tr className="border-b last:border-0">
      <th scope="row" className="py-2.5 pr-4 text-left text-sm font-medium">
        {pair.label}
        <span className="block font-mono text-xs font-normal text-muted-foreground">
          {pair.fg.slice(2)} on {pair.bg.map(b => b.slice(2)).join(" + ")}
        </span>
      </th>
      <td className="py-2.5 pr-4">
        <span
          aria-hidden
          className="inline-flex h-9 min-w-16 items-center justify-center rounded-md border border-ink-500/30 px-3 font-semibold"
          style={{ color: fg || undefined, background: bg || undefined }}
        >
          Aa
        </span>
      </td>
      <td className="py-2.5 pr-4 text-right font-mono text-sm tabular-nums">
        {mounted ? `${ratio.toFixed(2)}:1` : "…"}
      </td>
      <td className="py-2.5 text-right">
        {mounted ? (
          <span
            className={cn(
              "inline-flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-xs whitespace-nowrap",
              pass
                ? "border-status-live/35 bg-status-live/10 text-status-live"
                : "border-destructive/40 bg-destructive/10 text-destructive",
            )}
          >
            {pass ? <Check aria-hidden className="size-3" /> : <X aria-hidden className="size-3" />}
            {pass ? "AA" : "Fail"} ≥ {pair.min}
          </span>
        ) : null}
      </td>
    </tr>
  );
}

export function ContrastTable({ theme }: { theme: Theme }) {
  return (
    <Scroller label="Contrast checks">
      <table className="w-full min-w-[30rem] text-sm">
        <caption className="sr-only">
          WCAG contrast of text and focus colours in the {theme} theme
        </caption>
        <thead>
          <tr className="border-b font-mono text-label text-muted-foreground uppercase">
            <th scope="col" className="py-2 pr-4 text-left font-medium">Pair</th>
            <th scope="col" className="py-2 pr-4 text-left font-medium">Sample</th>
            <th scope="col" className="py-2 pr-4 text-right font-medium">Ratio</th>
            <th scope="col" className="py-2 text-right font-medium">Result</th>
          </tr>
        </thead>
        <tbody>
          {contrastPairs.pairs.map(pair => (
            <ContrastRow key={pair.label} theme={theme} pair={pair} />
          ))}
        </tbody>
      </table>
    </Scroller>
  );
}

export function Foundations() {
  const names = usePrimitiveNames();
  const mounted = useMounted();

  return (
    <>
      <ShowcaseSection
        id="colour"
        title="Colour"
        description="Primitives are fixed values; components only use the semantic tokens, which change with the theme. The default Tailwind palette is switched off, so these are the only colours available."
      >
        {scales.map(scale => (
          <Subsection key={scale.name} title={scale.name} description={scale.note}>
            <ul className="grid grid-cols-3 gap-3 sm:grid-cols-6 lg:grid-cols-12">
              {scale.steps.map(token => (
                <Swatch key={token} token={token} />
              ))}
            </ul>
          </Subsection>
        ))}

        <Subsection
          title="Semantic tokens"
          description="shadcn/ui names plus brand and status roles. Each maps to a primitive per theme. Use them as Tailwind utilities, for example bg-card, text-muted-foreground or border-brand-border."
        >
          <Scroller label="Semantic tokens">
            <table className="w-full min-w-[40rem] text-sm">
              <caption className="sr-only">Semantic colour tokens in both themes</caption>
              <thead>
                <tr className="border-b font-mono text-label text-muted-foreground uppercase">
                  <th scope="col" className="py-2 pr-4 text-left font-medium">Token</th>
                  <th scope="col" className="py-2 pr-4 text-left font-medium">Role</th>
                  <th scope="col" className="py-2 pr-4 text-left font-medium">Dark</th>
                  <th scope="col" className="py-2 text-left font-medium">Light</th>
                </tr>
              </thead>
              {semantic.map(group => (
                <tbody key={group.group}>
                  <tr>
                    <th
                      scope="colgroup"
                      colSpan={4}
                      className="pt-5 pb-2 text-left font-mono text-xs font-medium text-brand-text"
                    >
                      {group.group}
                    </th>
                  </tr>
                  {group.tokens.map(([token, role]) => (
                    <tr key={token} className="border-b">
                      <th scope="row" className="py-2 pr-4 text-left font-mono text-xs font-medium">
                        {token}
                      </th>
                      <td className="py-2 pr-4 text-muted-foreground">{role}</td>
                      <td className="py-2 pr-4">
                        <SemanticValue theme="dark" token={token} names={names} />
                      </td>
                      <td className="py-2">
                        <SemanticValue theme="light" token={token} names={names} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              ))}
            </table>
          </Scroller>
        </Subsection>

        <Subsection
          title="Contrast"
          description="Measured in the browser from the rendered colours. The same pairs are checked against tokens.css by npm run check:contrast. Normal text needs 4.5:1 (WCAG 2.2 AA); the focus ring is a UI component and needs 3:1."
        >
          <ThemePair>
            {/* The panel's own theme class scopes the colours. */}
            <ThemeAwareContrast />
          </ThemePair>
        </Subsection>
      </ShowcaseSection>

      <ShowcaseSection
        id="type"
        title="Typography"
        description="Inter for text and JetBrains Mono for labels, badges, hashes and numbers. Both are variable fonts, self-hosted with Latin and Latin Extended subsets."
      >
        <div className="grid gap-4 md:grid-cols-2">
          <div className="flex flex-col gap-2 rounded-xl border bg-card p-5">
            <p className="font-mono text-xs text-muted-foreground">font-sans · Inter Variable · 100–900</p>
            <p className="text-3xl font-semibold tracking-tight">Aa Bb Cc 0123456789</p>
            <p className="text-sm text-muted-foreground">
              <span className="font-light">Light 300</span> · <span>Regular 400</span> ·{" "}
              <span className="font-medium">Medium 500</span> ·{" "}
              <span className="font-semibold">Semibold 600</span> ·{" "}
              <span className="font-bold">Bold 700</span>
            </p>
          </div>
          <div className="flex flex-col gap-2 rounded-xl border bg-card p-5">
            <p className="font-mono text-xs text-muted-foreground">font-mono · JetBrains Mono Variable · 100–800</p>
            <p className="font-mono text-3xl font-semibold tracking-tight">Aa 0O 1lI 578136</p>
            <p className="font-mono text-sm break-all text-muted-foreground">
              3NKiFwqrZkMm4EUR9Ud1VJzWDoDL1uRgEYVADVigKm6adyhNW7SR
            </p>
          </div>
        </div>
        <ul className="flex flex-col divide-y rounded-xl border bg-card">
          {typeScale.map(row => (
            <li key={row.token} className="grid gap-2 p-5 md:grid-cols-[12rem_1fr] md:gap-6">
              <div className="flex flex-col gap-1">
                <p className="font-mono text-xs text-foreground">{row.token}</p>
                <p className="text-xs leading-relaxed text-muted-foreground">{row.note}</p>
              </div>
              <p className={cn("min-w-0 text-balance text-foreground", row.className)}>{row.sample}</p>
            </li>
          ))}
        </ul>
      </ShowcaseSection>

      <ShowcaseSection
        id="spacing"
        title="Spacing and layout"
        description="Tailwind's 4 px spacing scale (--spacing: 0.25rem), plus fluid section and gutter steps and two container widths."
      >
        <div className="grid gap-6 lg:grid-cols-2">
          <ul className="flex flex-col gap-2 rounded-xl border bg-card p-5">
            {spacingSteps.map(step => (
              <li key={step} className="grid grid-cols-[4.5rem_1fr] items-center gap-3">
                <span className="font-mono text-xs text-muted-foreground">
                  {step} · {step * 4}px
                </span>
                <span aria-hidden className="h-3 rounded-sm bg-brand" style={{ width: `${step * 4}px` }} />
              </li>
            ))}
          </ul>
          <ul className="flex flex-col gap-3 rounded-xl border bg-card p-5 text-sm">
            {[
              ["py-section", "--spacing-section", "Vertical space between page sections, 64–120 px fluid"],
              ["px-gutter", "--spacing-gutter", "Page side padding, 20–32 px fluid"],
              ["max-w-page", "--container-page", "Page content width, 76 rem (1216 px)"],
              ["max-w-prose", "--container-prose", "Reading width for paragraphs, 42 rem (672 px)"],
            ].map(([utility, token, note]) => (
              <li key={token} className="flex flex-col gap-0.5 border-b pb-3 last:border-0 last:pb-0">
                <span className="font-mono text-xs text-foreground">
                  {utility} <span className="text-muted-foreground">· {token}</span>
                </span>
                <span className="text-muted-foreground">{note}</span>
              </li>
            ))}
          </ul>
        </div>
      </ShowcaseSection>

      <ShowcaseSection
        id="radius"
        title="Radius"
        description="One base radius, --radius: 0.75rem (12 px), with steps derived from it, as in shadcn/ui."
      >
        <ul className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
          {radii.map(([utility, token]) => (
            <li key={utility} className="flex flex-col gap-2">
              <span aria-hidden className={cn("h-20 border border-brand-border bg-brand-subtle", utility)} />
              <span className="font-mono text-xs text-foreground">{utility}</span>
              <span className="font-mono text-xs text-muted-foreground">
                {token ? (mounted ? rawToken("dark", token) || token : token) : "9999px"}
              </span>
            </li>
          ))}
        </ul>
      </ShowcaseSection>

      <ShowcaseSection
        id="effects"
        title="Effects"
        description="Used sparingly: a hairline inset on cards, a teal glow under the primary button, and the hero's glow, grid and gradient text."
      >
        <ThemePair>
          <div className="grid gap-4 sm:grid-cols-2">
            <Specimen label="shadow-card">
              <div className="h-20 w-full rounded-xl border bg-card shadow-card" />
            </Specimen>
            <Specimen label="shadow-glow (primary button)">
              <Button>Primary action</Button>
            </Specimen>
            <Specimen label="bg-hero-glow">
              <div className="h-20 w-full rounded-xl border bg-background bg-hero-glow" />
            </Specimen>
            <Specimen label="bg-grid">
              <div className="h-20 w-full rounded-xl border bg-grid" />
            </Specimen>
            <div className="sm:col-span-2">
              <Specimen label="text-gradient-brand">
                <p className="text-title text-gradient-brand">A full node in the browser.</p>
              </Specimen>
            </div>
          </div>
        </ThemePair>
      </ShowcaseSection>

      <ShowcaseSection
        id="motion"
        title="Motion"
        description="Short and functional: colour and border transitions on hover and focus, and one pulsing dot for live status. With prefers-reduced-motion: reduce, animations and transitions are switched off globally."
      >
        <ul className="grid gap-4 sm:grid-cols-3">
          {[
            ["duration-150 · --duration-fast 120 ms", "Hover and focus colour changes"],
            ["--duration-base · 200 ms", "Larger state changes, such as opening a menu"],
            ["ease-standard", "cubic-bezier(0.2, 0, 0, 1)"],
          ].map(([name, note]) => (
            <li key={name} className="flex flex-col gap-1 rounded-xl border bg-card p-5">
              <span className="font-mono text-xs text-foreground">{name}</span>
              <span className="text-sm text-muted-foreground">{note}</span>
            </li>
          ))}
        </ul>
        <Specimen label="Live status pulse (motion-safe only)">
          <span className="inline-flex items-center gap-2 text-sm">
            <StatusDot status="live" pulse /> Following the tip
          </span>
        </Specimen>
      </ShowcaseSection>
    </>
  );
}

/** The contrast table for the enclosing ThemePair panel's theme. */
function ThemeAwareContrast() {
  return <ContrastTable theme={useThemePanel() ?? "dark"} />;
}
