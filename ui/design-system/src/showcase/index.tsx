"use client";

import * as React from "react";

import { Badge } from "@minanode/design-system/components/badge";
import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";

import { AppPatterns } from "./app-patterns";
import { ComponentGallery } from "./components";
import { Foundations } from "./foundations";
import { CodeBlock, ShowcaseSection } from "./layout";
import { showcaseSections } from "./sections";


const accessibility = [
  ["Contrast", "Every text pair in the colour section meets WCAG 2.2 AA (4.5:1) in both themes, and the focus ring meets 3:1. Brand teal #12A594 passes on the dark surfaces (6.0:1 or more); white text on teal does not (3.1:1), so primary buttons use dark text on teal in both themes, and teal text in the light theme uses teal-700."],
  ["Focus", "Every interactive element shows a 2 px ring in --ring, offset by 2 px, on keyboard focus only (focus-visible). Nothing removes the outline without replacing it."],
  ["Keyboard", "A skip link comes first. Tabs use arrow keys. The mobile menu is a disclosure button with aria-expanded, and Escape closes it. Scrollable tables and code are focusable regions with names."],
  ["Motion", "prefers-reduced-motion: reduce switches off animations and transitions everywhere, including smooth scrolling and the status pulse."],
  ["Meaning", "Status is never colour alone: each status badge carries its words. Icons are decorative (aria-hidden) unless they are the only label, which then goes in aria-label."],
  ["Targets", "Buttons are at least 32 px tall (icon-sm), above the WCAG 2.2 AA minimum of 24 px."],
];

export { showcaseSections };

export function DesignSystemShowcase() {
  return (
    <div className="flex flex-col">
      <header className="flex flex-col gap-5 pt-14 pb-10">
        <Eyebrow>Design system · v0.1.0</Eyebrow>
        <h1 className="text-display text-foreground">minanode design system</h1>
        <p className="max-w-prose text-lead text-pretty text-muted-foreground">
          The tokens and components shared by minanode.org and the node dashboard. Dark is the
          default theme; every component is shown in both. Values on this page are read from the
          live CSS, so it always matches the code. Numbers in the samples are placeholders, not
          claims.
        </p>
        <ul aria-label="Stack" className="flex flex-wrap gap-2">
          {["Tailwind CSS v4", "shadcn/ui · new-york", "Radix UI", "Inter", "JetBrains Mono"].map(item => (
            <li key={item}>
              <Badge variant="outline">{item}</Badge>
            </li>
          ))}
        </ul>
        <nav aria-label="Design system sections" className="pt-2">
          <ul className="flex flex-wrap gap-x-1 gap-y-1">
            {showcaseSections.map(item => (
              <li key={item.href}>
                <a
                  href={item.href}
                  className="inline-flex rounded-md px-2.5 py-1.5 text-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  {item.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
      </header>

      <Foundations />
      <ComponentGallery />
      <AppPatterns />

      <ShowcaseSection
        id="accessibility"
        title="Accessibility"
        description="The rules every component and page follows. The contrast measurements are in the colour section."
      >
        <ul className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {accessibility.map(([title, body]) => (
            <li key={title} className="flex flex-col gap-2 rounded-xl border bg-card p-5">
              <h3 className="font-semibold text-foreground">{title}</h3>
              <p className="text-sm leading-relaxed text-muted-foreground">{body}</p>
            </li>
          ))}
        </ul>
      </ShowcaseSection>

      <ShowcaseSection
        id="usage"
        title="Usage"
        description="The package ships source: TypeScript components and Tailwind CSS. The consuming app compiles them, so there is no build step and no drift between design and code. Add it as an npm workspace dependency."
      >
        <div className="grid gap-4 lg:grid-cols-2">
          <CodeBlock
            title="app/globals.css (Next.js) or src/index.css (Vite)"
            code={`@import "@minanode/design-system/styles.css";

/* App-specific styles go below. Tailwind scans the
   app's own files automatically; styles.css adds
   the design system's components. */`}
          />
          <CodeBlock
            title="Components"
            code={`import { Button } from "@minanode/design-system/components/button";
import { StatusBadge } from "@minanode/design-system/patterns/status-badge";

<StatusBadge status="live">Synced</StatusBadge>
<Button variant="outline">Open logs</Button>`}
          />
          <CodeBlock
            title="vite.config.ts (dashboard)"
            code={`import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
});`}
          />
          <CodeBlock
            title="index.html: dark by default, stored choice applied before paint"
            code={`<html lang="en" class="dark">
  <head>
    <script>/* themeInitScript from
      @minanode/design-system/lib/theme */</script>
  </head>
</html>`}
          />
          <CodeBlock
            title="Adding a shadcn component"
            code={`cd ui/design-system
npx shadcn@latest add dialog
# Writes src/components/dialog.tsx using the aliases
# in components.json. Then replace any dark: utilities
# with tokens so subtree theming keeps working.`}
          />
          <CodeBlock
            title="Checks"
            code={`npm run check:contrast   # AA pairs, both themes
npm run typecheck        # every workspace
npm run build            # static export to minanode-site/out`}
          />
        </div>
      </ShowcaseSection>
    </div>
  );
}
