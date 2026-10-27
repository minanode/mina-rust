"use client";

import * as React from "react";
import { Box, Cpu, LayoutDashboard, Lock, Network, RefreshCw, TriangleAlert, Wallet } from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import { Card, CardContent, CardHeader, CardTitle } from "@minanode/design-system/components/card";
import { Skeleton } from "@minanode/design-system/components/skeleton";
import { Amount } from "@minanode/design-system/patterns/amount";
import { SidebarNav, SidebarNavItem } from "@minanode/design-system/patterns/app-shell";
import { Banner } from "@minanode/design-system/patterns/banner";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { DataAge } from "@minanode/design-system/patterns/data-age";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { LoadingCard } from "@minanode/design-system/patterns/loading-card";
import { Logo } from "@minanode/design-system/patterns/logo";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { RelativeTime, TimeAgo, TimeUntil } from "@minanode/design-system/patterns/relative-time";
import { RoleCard } from "@minanode/design-system/patterns/role-card";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { Signal } from "@minanode/design-system/patterns/signal";
import { StageBar } from "@minanode/design-system/patterns/stage-bar";
import { StatRow } from "@minanode/design-system/patterns/stat-row";
import { TextLink } from "@minanode/design-system/patterns/text-link";
import { Timeline } from "@minanode/design-system/patterns/timeline";

import { ShowcaseSection, Specimen, Subsection, ThemePair, useThemePanel } from "./layout";

// Sample content here is placeholder text for showing patterns, not claims.

/** Renders its children only in the browser: the samples below count from
 * the current time, which the static export doesn't know. */
function InBrowser({ children, rows = 1 }: { children: React.ReactNode; rows?: number }) {
  const [mounted, setMounted] = React.useState(false);
  React.useEffect(() => setMounted(true), []);
  if (mounted) return <>{children}</>;
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-5 w-48" />
      ))}
    </div>
  );
}

/** A time `offsetMs` from when the specimen first rendered. */
function useSince(offsetMs: number) {
  const [at] = React.useState(() => Date.now() + offsetMs);
  return at;
}

function DataAgeStates() {
  const fresh = useSince(-3_000);
  const old = useSince(-125_000);
  return (
    <div className="flex flex-col gap-2">
      <Specimen label="loading · no data yet">
        <DataAge updatedAt={null} loading staleAfterMs={20_000} />
        <DataAge updatedAt={null} staleAfterMs={20_000} />
      </Specimen>
      <Specimen label="fresh, with refresh · stale · failed · paused">
        <DataAge updatedAt={fresh} staleAfterMs={20_000} onRefresh={() => {}} />
        <DataAge updatedAt={old} staleAfterMs={20_000} />
        <DataAge updatedAt={fresh} failed staleAfterMs={20_000} />
        <DataAge updatedAt={old} paused staleAfterMs={20_000} />
      </Specimen>
    </div>
  );
}

function TimeSamples() {
  const before = useSince(-45_000);
  const longBefore = useSince(-(2 * 3600 + 5 * 60) * 1000);
  const after = useSince((3 * 60 + 20) * 1000);
  return (
    <>
      <Specimen label="TimeAgo · TimeUntil · unknown">
        <TimeAgo at={before} />
        <TimeAgo at={longBefore} />
        <TimeUntil at={after} />
        <TimeUntil at={before} />
        <TimeAgo at={null} />
      </Specimen>
      <Specimen label="RelativeTime · past, future">
        <RelativeTime at={before} />
        <RelativeTime at={after} />
      </Specimen>
    </>
  );
}

function SegmentedSample() {
  const [value, setValue] = React.useState<"connected" | "all">("connected");
  return (
    <SegmentedControl
      label="Which peers"
      options={[
        { value: "connected", label: "Connected (12)" },
        { value: "all", label: "All (40)" },
      ]}
      value={value}
      onValueChange={setValue}
    />
  );
}

/** Visible text, with the panel's theme added for screen readers: a section
 * shown once per theme then keeps a unique name. */
function ThemedText({ text }: { text: string }) {
  const theme = useThemePanel();
  return (
    <>
      {text}
      {theme ? <span className="sr-only"> ({theme})</span> : null}
    </>
  );
}

const SLOT_TICKS = [0, 30, 60, 90].map(at => ({ at, label: at === 0 ? "0" : `${at} s` }));

function StageSamples() {
  return (
    <div className="flex flex-col gap-5">
      <Specimen label="done: built, proved, sent early in the window">
        <StageBar
          className="w-full max-w-md"
          label="Sample: a 90 s window; built at −6 s, proved for 9 s, sent at +4 s."
          domain={[-10, 90]}
          window={{ start: 0, end: 90 }}
          spans={[
            { start: -6, end: -5.7, tone: "neutral" },
            { start: -5.7, end: 3, tone: "info" },
          ]}
          marks={[{ at: 4, tone: "ok" }]}
          ticks={SLOT_TICKS}
        />
      </Specimen>
      <Specimen label="stopped: dropped during the second span">
        <StageBar
          className="w-full max-w-md"
          label="Sample: a 90 s window; built at −6 s, dropped at +1.5 s while proving."
          domain={[-10, 90]}
          window={{ start: 0, end: 90 }}
          spans={[
            { start: -6, end: -5.7, tone: "neutral" },
            { start: -5.7, end: 1.5, tone: "warn" },
          ]}
          marks={[{ at: 1.5, tone: "warn" }]}
          ticks={SLOT_TICKS}
        />
      </Specimen>
      <Specimen label="Timeline · done, current, stopped, skipped, pending">
        <div className="grid w-full gap-6 sm:grid-cols-2">
          <Timeline
            label="Sample steps, one stopped"
            steps={[
              { label: "First step", time: "−6.0 s", state: "done", detail: "A line of detail." },
              { label: "Second step", time: "−5.7 s", state: "done" },
              { label: "Third step", state: "stopped" },
              { label: "Fourth step", state: "skipped" },
              { label: "Outcome", time: "+1.5 s", state: "done", tone: "warn", detail: "Why it ended here." },
            ]}
          />
          <Timeline
            label="Sample steps, under way"
            steps={[
              { label: "First step", time: "44 min before", state: "done" },
              { label: "Second step", state: "current" },
              { label: "Third step", state: "pending" },
            ]}
          />
        </div>
      </Specimen>
    </div>
  );
}

function PageHeaderSample() {
  const theme = useThemePanel();
  return (
    <PageHeader
      titleId={`showcase-page-title-${theme}`}
      eyebrow="Roles"
      title="Page title"
      description="A lead that says what the page shows and where the figures come from."
      aside={
        <>
          <Signal tone="ok" className="text-sm">
            On
          </Signal>
          <Button type="button" variant="outline" size="sm">
            <RefreshCw aria-hidden />
            Action
          </Button>
        </>
      }
    />
  );
}

export function AppPatterns() {
  return (
    <ShowcaseSection
      id="app-patterns"
      title="App patterns"
      description="The frame and page parts of an app that reads a node: the node dashboard (ui/dashboard) is built from these, and any other app can be. They know nothing about the node; @minanode/node-client brings the data."
    >
      <Subsection
        title="App shell"
        description="AppShell is the frame: a skip link, a sidebar from 1024 px with the brand, a SidebarNav and a footer, a header with the app's status and, below 1024 px, a menu button that opens the same navigation (Escape closes it). A Banner goes under the header. Its landmarks make it a whole page, so here are its parts; the dashboard shows it whole. Routing stays the app's: put its links in SidebarNavItem asChild."
      >
        <ThemePair>
          <div className="grid overflow-hidden rounded-lg border sm:grid-cols-[13rem_minmax(0,1fr)]">
            <div className="flex flex-col gap-4 border-b bg-card/40 p-3 sm:border-r sm:border-b-0">
              <div className="flex flex-col gap-1 px-2 pt-2">
                <Logo />
                <p className="pl-[2.6rem] font-mono text-label text-muted-foreground uppercase">App name</p>
              </div>
              <SidebarNav>
                <SidebarNavItem href="#app-patterns" current>
                  <LayoutDashboard aria-hidden />
                  Current page
                </SidebarNavItem>
                <SidebarNavItem href="#app-patterns">
                  <Cpu aria-hidden />
                  Rest
                </SidebarNavItem>
                <SidebarNavItem href="#app-patterns" data-preview="hover">
                  <Wallet aria-hidden />
                  Hover
                </SidebarNavItem>
                <SidebarNavItem href="#app-patterns" data-preview="focus">
                  <Network aria-hidden />
                  Focus
                </SidebarNavItem>
              </SidebarNav>
            </div>
            <div className="flex min-w-0 flex-col">
              <div className="flex h-14 items-center justify-end border-b px-4">
                <Signal tone="ok">
                  Synced <span className="text-foreground/80">578,883</span>
                </Signal>
              </div>
              <Banner title="The source isn't answering.">Figures are from 2 min ago.</Banner>
              <div className="flex flex-col gap-2 p-4">
                <Skeleton className="h-6 w-40" />
                <Skeleton className="h-4 w-full" />
              </div>
            </div>
          </div>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Banner"
        description="A full-width line under the header for something every page must know. Announced once (role=alert). Tones: destructive (default) for something broken, caution for something to watch."
      >
        <ThemePair layout="stack">
          <Banner title="The node isn't answering.">Figures on this page are from 2 min ago.</Banner>
          <Banner tone="caution" title="The node is catching up.">
            Figures may lag the chain until it's synced.
          </Banner>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Page header and page section"
        description="PageHeader tops a page: eyebrow, the page's h1 (it takes focus after an in-app navigation, by titleId), a lead, and controls on the right. PageSection is a titled part of the page, an h2 with a description and controls, named by its heading."
      >
        <ThemePair layout="stack">
          <PageHeaderSample />
          <PageSection
            title={<ThemedText text="Section title" />}
            description="What this part of the page shows."
            aside={
              <TextLink href="#app-patterns" arrow className="text-sm">
                Related page
              </TextLink>
            }
          >
            <div className="rounded-xl border bg-card p-5 text-sm text-muted-foreground">Section content.</div>
          </PageSection>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Data age"
        description="How old a page's figures are, counting: muted while fresh, amber once the data is older than staleAfterMs or the last request failed, and saying so when polling is paused on purpose. onRefresh adds a refresh button."
      >
        <ThemePair>
          <InBrowser rows={2}>
            <DataAgeStates />
          </InBrowser>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Loading card"
        description="Placeholder rows while the first answer is on its way; screen readers hear the label. Later answers keep the old figures on screen and let the data age say how old they are."
      >
        <ThemePair>
          <LoadingCard rows={3} label="Loading the peer list" />
        </ThemePair>
      </Subsection>

      <Subsection
        title="Role card"
        description="Something the app's subject does, as a card: icon, title, state on the right (a Signal, or a Skeleton while loading), a few lines, and a TextLink with an arrow to its page."
      >
        <ThemePair layout="stack">
          <div className="grid gap-4 md:grid-cols-3">
            <RoleCard
              icon={<Cpu />}
              title="On"
              status={<Signal tone="ok">Working</Signal>}
              action={
                <TextLink href="#app-patterns" arrow className="text-sm font-medium">
                  Manage it
                </TextLink>
              }
            >
              <span>A sentence about what it is doing.</span>
              <span>A second line with a figure: 12 proofs made.</span>
            </RoleCard>
            <RoleCard
              icon={<Box />}
              title="Off"
              status={<Signal tone="neutral">Off</Signal>}
              action={
                <TextLink href="#app-patterns" arrow className="text-sm font-medium">
                  How to turn it on
                </TextLink>
              }
            >
              <span>It doesn't do this.</span>
            </RoleCard>
            <RoleCard icon={<Box />} title="Loading" status={<Skeleton className="h-6 w-14 rounded-full" />}>
              <span>Loading…</span>
            </RoleCard>
          </div>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Stat row"
        description="A row of labelled figures with hairlines between them, as a description list: a period's counts and times. It wraps into as many columns as fit (minItemWidth, 9.5rem by default). A box of its own; inside a Card it drops its border."
      >
        <ThemePair layout="stack">
          <StatRow
            items={[
              { label: "First", value: "12", detail: "a short caveat" },
              { label: "Second", value: "9" },
              { label: "Third", value: "75%", detail: "of the first" },
              { label: "Fourth", value: "0" },
              { label: "A time", value: "9.1 s", detail: "the median" },
              { label: "An amount", value: <Amount value="1,080" unit="MINA" /> },
            ]}
          />
        </ThemePair>
      </Subsection>

      <Subsection
        title="Stage bar and timeline"
        description="StageBar puts the stages of one run on a window of time: the window lit on a track, spans for stages that took time, dots for moments, ticks below. It is one image to assistive tech, named by label, so say there what it shows and give the figures in words too. Timeline lists the same stages in order, each with a dot for how it went (done, under way, stopped, not reached, to come), the state in words, its time and a line of detail."
      >
        <ThemePair>
          <StageSamples />
        </ThemePair>
      </Subsection>

      <Subsection
        title="One level of boxes"
        description="A bordered surface (a Card, a KeyValueList) is the box. An empty state, an alert or an inline confirmation inside it has no border of its own (lib/surface): the empty state goes flat, the others keep only their tint. On its own, an empty state is a dashed box."
      >
        <ThemePair>
          <Card className="gap-4">
            <CardHeader>
              <CardTitle>A card</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <EmptyState
                icon={<Lock />}
                title="An empty state, flat"
                description={<p>What to do, without a box around it.</p>}
              />
              <Alert variant="caution">
                <TriangleAlert aria-hidden />
                <AlertTitle>An alert, tint only</AlertTitle>
                <AlertDescription>
                  <p>No border inside the card.</p>
                </AlertDescription>
              </Alert>
              <InlineConfirm
                label="Confirm inside a card (sample)"
                message="An inline confirmation, tint only."
                confirmLabel="Confirm"
                onConfirm={() => {}}
                onCancel={() => {}}
              />
            </CardContent>
          </Card>
          <EmptyState
            icon={<Lock />}
            title="An empty state on its own"
            description={<p>A dashed box: the page has no other around it.</p>}
          />
        </ThemePair>
      </Subsection>

      <Subsection
        title="Inline confirm"
        description="A confirmation that opens in place for an action the page can't undo: what will happen, a destructive confirm, a way back. While busy, both buttons are disabled and the confirm shows busyLabel."
      >
        <ThemePair>
          <InlineConfirm
            label="Confirm stopping (sample)"
            message="Stop it? Work in progress is dropped."
            confirmLabel="Stop"
            cancelLabel="Keep it running"
            onConfirm={() => {}}
            onCancel={() => {}}
          />
          <InlineConfirm
            label="Confirm stopping, busy (sample)"
            message="Stop it? Work in progress is dropped."
            confirmLabel="Stop"
            busyLabel="Stopping…"
            cancelLabel="Keep it running"
            busy
            onConfirm={() => {}}
            onCancel={() => {}}
          />
        </ThemePair>
      </Subsection>

      <Subsection
        title="Copy button"
        description="Copies text on https and on plain http (a fallback for the Clipboard API, which browsers only offer in secure contexts). Named for what it copies; says it copied, or that it couldn't and the text is there to select. Hash, CodeBlock and JsonTree use it."
      >
        <ThemePair>
          <Specimen label="icon sm · idle, copied, failed">
            <CopyButton text="sample" label="sample key" />
            <CopyButton text="sample" label="sample key" state="copied" />
            <CopyButton text="sample" label="sample key" state="failed" />
          </Specimen>
          <Specimen label="icon md · idle, copied, failed">
            <CopyButton size="md" text="sample" label="sample command" />
            <CopyButton size="md" text="sample" label="sample command" state="copied" />
            <CopyButton size="md" text="sample" label="sample command" state="failed" />
          </Specimen>
          <Specimen label="text · idle, copied, failed">
            <CopyButton variant="text" text="{}" label="sample JSON">
              Copy JSON
            </CopyButton>
            <CopyButton variant="text" text="{}" label="sample JSON" state="copied">
              Copy JSON
            </CopyButton>
            <CopyButton variant="text" text="{}" label="sample JSON" state="failed">
              Copy JSON
            </CopyButton>
          </Specimen>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Segmented control"
        description="A few exclusive choices that change what a page shows: toggle buttons (aria-pressed) in a named group. For panels of different content, use Tabs."
      >
        <ThemePair>
          <SegmentedSample />
        </ThemePair>
      </Subsection>

      <Subsection
        title="Text link"
        description="An in-app link in brand teal, underlined on hover. arrow adds an arrow for go-to-that-page links; asChild styles the app's router link. Links that leave the site use ExternalLink."
      >
        <ThemePair>
          <Specimen label="rest · hover · focus · arrow">
            <TextLink href="#app-patterns">Rest</TextLink>
            <TextLink href="#app-patterns" data-preview="hover">
              Hover
            </TextLink>
            <TextLink href="#app-patterns" data-preview="focus">
              Focus
            </TextLink>
            <TextLink href="#app-patterns" arrow>
              With arrow
            </TextLink>
          </Specimen>
        </ThemePair>
      </Subsection>

      <Subsection
        title="Time and amount"
        description="TimeAgo and TimeUntil count from the current time, with the UTC time in the title; RelativeTime picks ago or in, for a time that can be either side of now (TimeAgo would read a future time as 0 s ago). Amount sets a formatted figure with its unit muted. Unknown values show a dash."
      >
        <ThemePair>
          <InBrowser>
            <TimeSamples />
          </InBrowser>
          <Specimen label="Amount · unknown">
            <Amount value="0.004" unit="MINA" />
            <Amount value="1,234.5" unit="MINA" />
            <Amount value={null} unit="MINA" />
          </Specimen>
        </ThemePair>
      </Subsection>
    </ShowcaseSection>
  );
}
