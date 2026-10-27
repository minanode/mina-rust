# @minanode/design-system

The tokens, components and patterns shared by minanode.org and the node
dashboard. It ships source: TypeScript components and Tailwind v4 CSS that the
consuming app compiles. Every component and pattern below appears on the
design-system page (`/design-system/` on the site) in both themes and in all its
states. That page's source is `src/showcase/`.

## Use it in an app

1. Add `"@minanode/design-system": "0.1.0"` to the app's `dependencies` and the
   app to `workspaces` in `ui/package.json`.
2. Compile it with Tailwind v4: `@tailwindcss/vite` for Vite, or
   `@tailwindcss/postcss` for Next.js.
3. In the app's CSS, add `@import "@minanode/design-system/styles.css";`. It
   brings the tokens, fonts and base styles, and makes Tailwind scan the design
   system's files.
4. Theme: put `class="dark"` on `<html>` (dark is the default) and run
   `themeInitScript` from `lib/theme` in the head, so a stored light choice
   applies before paint.
5. Import by path: `@minanode/design-system/components/button`,
   `@minanode/design-system/patterns/page-header`,
   `@minanode/design-system/lib/time`.

The package depends on React 19, Radix UI and lucide-react. Components take
every theme difference from the tokens, so `.dark` or `.light` on any element
themes that subtree.

## Components (`components/`)

shadcn/ui (new-york) components, restyled through the tokens. Their props are
shadcn's. Add more with `npx shadcn@latest add <name>` in this directory, then
replace any `dark:` utilities they bring with tokens.

- **Button**: `variant` default, secondary, outline, ghost, link or destructive;
  `size` sm, default, lg, icon or icon-sm; `asChild` styles a link.
  `<Button variant="outline" size="sm">Refresh</Button>`
- **Badge**: `variant` default, solid, secondary, outline or destructive; `font`
  sans or mono. `<Badge variant="default">devnet</Badge>`
- **Card**: `Card`, `CardHeader`, `CardTitle`, `CardDescription`, `CardAction`,
  `CardContent`, `CardFooter`.
- **Tabs**: Radix tabs, for panels of different content.
  `<Tabs defaultValue="a"><TabsList><TabsTrigger value="a">…`
- **Table**: `Table` (with `containerProps` for its scrolling wrapper),
  `TableHeader`, `TableBody`, `TableRow`, `TableHead`, `TableCell`,
  `TableCaption`. `tableRegion(label)` makes the wrapper a focusable, named
  region, so a keyboard can scroll it:
  `<Table containerProps={tableRegion("Best chain blocks")}>`.
- **Alert**: `variant` default, brand, caution or destructive; `AlertTitle`,
  `AlertDescription`. Inside a Card or a KeyValueList it keeps its tint but has
  no border.
- **Input**, **Textarea** (Input over several lines), **Label**, **Separator**
  (`orientation`, `decorative`), **Skeleton** (size it with classes:
  `<Skeleton className="h-5 w-48" />`).

## Patterns (`patterns/`)

Compositions built from the tokens. The node dashboard builds every page from
these and `@minanode/node-client`.

One level of boxes: a bordered surface (a Card, a KeyValueList) is the box, and
a box-like pattern inside it has no border of its own. An EmptyState goes flat
there (no border, fill or padding); an Alert and an InlineConfirm keep their
tint. The rule is in `lib/surface` (Tailwind's `in-data-[slot=card]` variants),
so it holds wherever a page puts them.

### Frame and page

**AppShell** (`app-shell`): an app's frame. It has a skip link, a sidebar from
1024 px (brand, navigation, footer), and a header with the app's status. Below
1024 px a menu button opens the same navigation: focus moves to its first link,
and Escape closes it and returns focus. A banner sits under the header, and the
page goes in `main#main`. Props: `brand`, `brandCaption`, `mobileBrand`,
`navLabel`, `nav`, `sidebarFooter`, `headerEnd`, `mobileHeaderEnd` and `banner`.
Routing stays the app's. `SidebarNavItem` takes `current` (sets
`aria-current="page"` and the brand fill) and `asChild` (styles your router's
link); a click closes the mobile menu.

```tsx
<AppShell
  brand={<Logo />}
  brandCaption="Node dashboard"
  navLabel="Dashboard"
  nav={
    <SidebarNav>
      <SidebarNavItem asChild current={page === "overview"}>
        <Link to="/">
          <LayoutDashboard aria-hidden />
          Overview
        </Link>
      </SidebarNavItem>
    </SidebarNav>
  }
  headerEnd={<Signal tone="ok">Synced</Signal>}
  banner={offline ? <Banner title="The node isn't answering." /> : null}
>
  {page}
</AppShell>
```

**Banner** (`banner`): a full-width line under the header for something every
page must know. It is announced once (`role="alert"`). Props: `title`,
`children` (the detail), and `tone`, either `destructive` (the default) or
`caution`.

**PageHeader** (`page-header`): the top of a page. Props: `eyebrow`, `title`
(the page's h1), `description`, `aside` (data age, buttons) and `titleId`
(default `page-title`; focus it after an in-app navigation).
`<PageHeader eyebrow="Roles" title="SNARK worker" aside={<DataAge … />} />`

**PageSection** (`page-section`): a titled part of a page, named by its h2.
Props: `title`, `description`, `aside`, `children`, `className`.

**SectionHeading** (`section-heading`) and **Eyebrow** (`eyebrow`): the site's
section titles (`eyebrow`, `title`, `description`, `as` h1, h2 or h3, `id`,
`align`) and the mono label above them.

### Live data

**DataAge** (`data-age`): how old the figures are, counting. It reads "Updated 3
s ago", turns amber as "Data from 2 min ago" once the data is older than
`staleAfterMs` or the last request `failed`, and reads "Paused, data from …"
when `paused`. Props: `updatedAt` (ms or null), `loading`, `failed`,
`staleAfterMs`, `paused`, `onRefresh` (adds a refresh button). With
`@minanode/node-client/react`:
`<DataAge updatedAt={q.updatedAt} loading={q.loading} failed={q.error !== null} staleAfterMs={20_000} onRefresh={q.refresh} />`

**LoadingCard** (`loading-card`): placeholder rows while the first answer is on
its way. Props: `rows` (default 3) and `label` (read to screen readers).

**Signal** (`signal`): the live state of something running. `tone` is ok, info,
warn, error or neutral; `pulse` is only for something in progress. Always give
it words: `<Signal tone="info" pulse>Catching up</Signal>`. `SignalDot` is the
dot alone, and `toneFill` maps a tone to its solid fill class, for dots and bars
that carry the tone next to words.

**StatusBadge** (`status-badge`): the site's evidence status. `status` is live,
devnet, pending or planned. `StatusDot` is the dot alone.

**TimeAgo** and **TimeUntil** (`relative-time`): "45 s ago" and "in 2 h 5 min",
counting, in a `<time>`. Prop: `at` (ms, or null for a dash). **RelativeTime**
picks "ago" or "in" for a time that can be either side of now, such as a won
slot; TimeAgo would read a future time as "0 s ago". The ages come from
`lib/time`: `formatAge(ms)` and `useNow(everyMs)`.

**Amount** (`amount`): a formatted figure with its unit muted, in mono. Props:
`value` (format it first; null shows a dash) and `unit`.
`<Amount value={formatMina(fee)} unit="MINA" />`

### Figures

**MetricCard** (`metric-card`): a number in a card with its label and caveat.
Props: `value`, `label`, `detail`, `footer`, and `size` default or sm.

**Stat** (`stat`): a number without a card, for rows of figures.
`variant="default"` puts the value first in teal; it shrinks to fit and never
wraps (the site). `variant="labelled"` puts a mono label first, the value in the
text colour, then a sentence of `detail` (a dashboard's live figures). `inList`
renders the labelled parts as `dt` and `dd` for use inside a `<dl>`, and
`size="sm"` sets a smaller labelled value.
`<dl><Stat variant="labelled" inList label="Peers" value="14" detail="connected" /></dl>`

**StatRow** (`stat-row`): a row of labelled figures with hairlines between them,
as one `<dl>`: a period's counts and times. It wraps into as many columns as
fit. Props: `items` (`{ label, value, detail? }[]`), `label` (names it as a
group, to tell rows apart) and `minItemWidth` (default `9.5rem`). A box of its
own; inside a Card or a KeyValueList it drops its border.

**StageBar** (`stage-bar`): the stages of one run on a window of time, on one
line: a track over `domain`, the `window` that matters lit on it, `spans`
(`{ start, end, tone? }`) for stages that took time, `marks` (`{ at, tone? }`)
for moments, and `ticks` (`{ at, label }`) below. Numbers are in any one unit.
It is one image to assistive tech, named by `label`: say there what it shows,
and give the figures in words nearby too. The dashboard draws each won slot's
build, proof and broadcast on its 90 s slot with it.

**Timeline** (`timeline`): steps in order, as an `<ol>` named by `label`. Each
step (`{ label, time?, detail?, state, tone? }`) has a dot for its `state`:
`done` (filled, in `tone`, ok by default), `current` (pulsing), `stopped`,
`skipped` or `pending` (hollow). The states that aren't done are also said in
words.

**KeyValueList** (`key-value-list`): a receipt of label and value rows in mono.
Props: `title`, `aside`, `rows` (`{ label, value, tone?: "brand" }[]`) and
`footer`.

**RoleCard** (`role-card`): something the app's subject does, as a card. Props:
`icon`, `title`, `status` (a Signal, or a Skeleton while loading), `children` (a
few lines) and `action` (a TextLink with an arrow).

### Values and actions

**Hash** (`hash`): hashes, keys and peer IDs in mono. When truncated it shows
the ends; the whole value goes in the title and to screen readers. Props:
`value`, `truncate` (default true), `head` and `tail` (default 6), `copy`
(default true) and `label` (names the copy button: "Copy public key").

**CopyButton** (`copy-button`): copies `text`. It uses the Clipboard API on
https or localhost, and `lib/clipboard`'s fallback on plain http. Its name says
what it copies (`label`), then that it copied or couldn't, and it goes back
after 2 s. `variant="icon"` with `size` sm (beside a value) or md (bordered,
over a block); `variant="text"` with `children` as its words. `state` forces a
state for the showcase. Hash, CodeBlock and JsonTree use it.
`<CopyButton text={key} label="public key" />`

**CodeBlock** (`code-block`): a command or snippet to copy, in a focusable,
named region that scrolls sideways. Props: `code` and `label` ("curl command").

**JsonTree** (`json-tree`): any JSON value as a collapsible tree. It keeps folds
by path while `value` changes, so a polled answer doesn't jump. Props: `value`,
`label` (names the region and the copy button), `openDepth` (default 2: the root
and its children), `pageSize` (default 50 items of a long array at a time) and
`copyable` (default true: Copy JSON, pretty-printed). Expand all and Collapse
all are in its toolbar. `<JsonTree value={answer} label="Node status" />`

**InlineConfirm** (`inline-confirm`): a confirmation that opens in place for an
action the page can't undo. Props: `label` (the group's name), `message`,
`confirmLabel`, `busyLabel`, `cancelLabel` (default Cancel), `busy` (disables
both buttons), `onConfirm` and `onCancel`.

```tsx
{
  confirming ? (
    <InlineConfirm
      label="Confirm stopping the worker"
      message="Stop the worker? A proof in progress is dropped."
      confirmLabel="Stop worker"
      busyLabel="Stopping…"
      cancelLabel="Keep it running"
      busy={sending}
      onConfirm={stop}
      onCancel={() => setConfirming(false)}
    />
  ) : null;
}
```

**SegmentedControl** (`segmented-control`): a few exclusive choices that change
what a page shows, as toggle buttons with `aria-pressed` in a named group. Use
Tabs instead for panels of different content. Props: `label`, `options`
(`{ value, label }[]`), `value` and `onValueChange`.

**EmptyState** (`empty-state`): what a panel says when it has nothing to show.
Props: `icon`, `title`, `description`, `action`, and `tone` default, caution or
destructive. On its own it is a dashed box; inside a Card it is flat.

### Links and brand

**TextLink** (`text-link`): an in-app link in brand teal, underlined on hover.
`arrow` adds an arrow for "go to that page"; `asChild` styles your router's
link. `<TextLink asChild arrow><Link to="/mempool">Mempool</Link></TextLink>`

**ExternalLink** (`external-link`): a link that opens a new tab and tells screen
readers it does. `icon` (default true) shows the arrow.

**Logo** and **LogoMark** (`logo`): the mark with the wordmark, or the mark
alone (`shape` rounded or square; at least 24 px).

**ThemeToggle** (`theme-toggle`): switches the whole page between dark and light
and remembers the choice (`lib/theme`).

## Library (`lib/`)

- `utils`: `cn(...classes)`, clsx with tailwind-merge.
- `theme`: `themeInitScript`, `applyTheme`, `THEME_STORAGE_KEY`.
- `time`: `formatAge(ms)` (`45 s`, `6 min 30 s`, `2 h 5 min`) and
  `useNow(everyMs = 1000)`.
- `clipboard`: `copyText(text)`, which resolves to whether it copied (with the
  plain-http fallback), and `copyName(state, what)`.
- `surface`: `borderlessInSurface` and `flatInSurface`, the classes that take a
  box's border (and, flat, its fill and padding) off inside a surface.

## Checks

From `ui/`: `npm run check:contrast` (AA pairs in both themes, from
`tokens.css`), `npm run typecheck`, and `npm test`. This package's Vitest tests
(jsdom, Testing Library) cover JsonTree, TextLink, InlineConfirm, CopyButton,
SegmentedControl, DataAge, RelativeTime, Stat, the surface rule and the AppShell
menu.
