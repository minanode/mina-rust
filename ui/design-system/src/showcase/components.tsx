"use client";

import * as React from "react";
import {
  ArrowRight,
  CircleSlash,
  Download,
  Info,
  Lock,
  Plus,
  TriangleAlert,
  Unplug,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@minanode/design-system/components/alert";
import { Badge } from "@minanode/design-system/components/badge";
import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Textarea } from "@minanode/design-system/components/textarea";
import { Separator } from "@minanode/design-system/components/separator";
import { Skeleton } from "@minanode/design-system/components/skeleton";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@minanode/design-system/components/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@minanode/design-system/components/tabs";
import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { JsonTree } from "@minanode/design-system/patterns/json-tree";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { Logo, LogoMark } from "@minanode/design-system/patterns/logo";
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { SectionHeading } from "@minanode/design-system/patterns/section-heading";
import { Signal } from "@minanode/design-system/patterns/signal";
import { Stat } from "@minanode/design-system/patterns/stat";
import { StatusBadge, StatusDot } from "@minanode/design-system/patterns/status-badge";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";

import {
  Scroller,
  ShowcaseSection,
  Specimen,
  Subsection,
  ThemePair,
  useThemedLabel,
} from "./layout";

// Sample content on this page is placeholder text for showing components,
// not claims. Claims live in the site's content and CLAIMS.md.

const buttonVariants = ["default", "secondary", "outline", "ghost", "link", "destructive"] as const;
const states = [
  { label: "Rest", props: {} },
  { label: "Hover", props: { "data-preview": "hover" } },
  { label: "Focus", props: { "data-preview": "focus" } },
  { label: "Active", props: { "data-preview": "active" } },
  { label: "Disabled", props: { disabled: true } },
] as const;

function ButtonStates() {
  return (
    <Scroller label="Button variants and states">
      <table className="w-full min-w-[38rem] border-separate border-spacing-y-2 text-sm">
        <caption className="sr-only">Button variants in each state</caption>
        <thead>
          <tr className="font-mono text-xs text-muted-foreground">
            <th scope="col" className="pr-4 text-left font-normal">variant</th>
            {states.map(state => (
              <th key={state.label} scope="col" className="px-2 text-left font-normal">
                {state.label.toLowerCase()}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {buttonVariants.map(variant => (
            <tr key={variant}>
              <th scope="row" className="pr-4 text-left font-mono text-xs font-normal text-muted-foreground">
                {variant}
              </th>
              {states.map(state => (
                <td key={state.label} className="px-2 py-1">
                  <Button variant={variant} {...state.props}>
                    {variant === "destructive" ? "Stop node" : "Button"}
                  </Button>
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </Scroller>
  );
}

const badgeVariants = ["default", "solid", "secondary", "outline", "destructive"] as const;

function ExampleTable({ children }: { children: React.ReactNode }) {
  const label = useThemedLabel("Example table");
  return (
    <Table
      containerProps={{ role: "region", "aria-label": label, tabIndex: 0 }}
      className="min-w-[32rem]"
    >
      {children}
    </Table>
  );
}

function TabStates() {
  const label = useThemedLabel("Tab states");
  return (
    <Tabs value="active">
      <TabsList aria-label={label}>
        <TabsTrigger value="a">Inactive</TabsTrigger>
        <TabsTrigger value="b" data-preview="hover">
          Hover
        </TabsTrigger>
        <TabsTrigger value="c" data-preview="focus">
          Focus
        </TabsTrigger>
        <TabsTrigger value="active">Active</TabsTrigger>
        <TabsTrigger value="d" disabled>
          Disabled
        </TabsTrigger>
      </TabsList>
      <TabsContent value="active" className="text-sm text-muted-foreground">
        Panel for the active tab.
      </TabsContent>
    </Tabs>
  );
}

function TextareaField({ label, ...props }: React.ComponentProps<typeof Textarea> & { label: string }) {
  const id = React.useId();
  return (
    <div className="flex min-w-48 flex-1 flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Textarea id={id} rows={3} {...props} />
    </div>
  );
}

function Field({
  label,
  ...props
}: React.ComponentProps<typeof Input> & { label: string }) {
  const id = React.useId();
  return (
    <div className="flex min-w-48 flex-1 flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input id={id} {...props} />
    </div>
  );
}

// Each sample renders once per theme panel; the panel's theme in the label
// keeps their regions' names unique.
function JsonTreeSample() {
  return (
    <JsonTree
      label={useThemedLabel("Sample answer")}
      pageSize={3}
      value={{
        network: "devnet",
        best_tip: { height: 578883, hash: "3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W" },
        synced: true,
        producer: null,
        peers: [{ id: "2aTKv6" }, { id: "2bSFf6" }, { id: "2buYjF" }, { id: "2acGWj" }, { id: "2aWLP4" }],
      }}
    />
  );
}

function CodeBlockSample() {
  return (
    <CodeBlock
      label={useThemedLabel("curl command")}
      code={`curl -s http://127.0.0.1:3000/graphql \\\n  -H 'content-type: application/json' \\\n  -d '{"query":"{ syncStatus }"}'`}
    />
  );
}

export function ComponentGallery() {
  return (
    <>
      <ShowcaseSection
        id="components"
        title="Components"
        description="shadcn/ui components (new-york style) restyled through the tokens. Each is shown in both themes; hover, focus and active are forced with data-preview so every state is visible without a pointer."
      >
        <Subsection
          title="Button"
          description="Six variants, five sizes. One primary action per view. Labels say what happens. For a link styled as a button, use asChild with an <a>. Do not use a disabled button for something that is not available yet: say so in text or a status badge instead."
        >
          <ThemePair layout="stack">
            <ButtonStates />
            <Specimen label="sizes">
              <Button size="sm">Small</Button>
              <Button>Default</Button>
              <Button size="lg">Large</Button>
              <Button size="icon" aria-label="Add">
                <Plus />
              </Button>
              <Button size="icon-sm" variant="outline" aria-label="Add">
                <Plus />
              </Button>
            </Specimen>
            <Specimen label="with icons · asChild link">
              <Button>
                <Download /> Download
              </Button>
              <Button variant="outline" asChild>
                <a href="#components">
                  Read more <ArrowRight />
                </a>
              </Button>
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Badge"
          description="Short labels and identifiers. Monospace by default; use font=&quot;sans&quot; for words. As a link (asChild with <a>) it gets a hover state."
        >
          <ThemePair>
            <Specimen label="variants · mono">
              {badgeVariants.map(variant => (
                <Badge key={variant} variant={variant}>
                  {variant}
                </Badge>
              ))}
            </Specimen>
            <Specimen label="variants · sans">
              {badgeVariants.map(variant => (
                <Badge key={variant} variant={variant} font="sans">
                  {variant}
                </Badge>
              ))}
            </Specimen>
            <Specimen label="link · rest, hover, focus">
              {(["", "hover", "focus"] as const).map(state => (
                <Badge key={state} asChild>
                  <a href="#components" data-preview={state || undefined}>
                    #578136
                  </a>
                </Badge>
              ))}
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Status badge"
          description="Where evidence stands, using the four status tokens. Always pair the colour with words; the dot alone is decorative. The pulse is for live data only and stops under reduced motion."
        >
          <ThemePair>
            <Specimen label="status">
              <StatusBadge status="live">Working, on main</StatusBadge>
              <StatusBadge status="devnet">Shown on public devnet</StatusBadge>
              <StatusBadge status="pending">In preparation</StatusBadge>
              <StatusBadge status="planned">Coming soon</StatusBadge>
            </Specimen>
            <Specimen label="pulse · dot">
              <StatusBadge status="live" pulse>
                Synced
              </StatusBadge>
              <span className="inline-flex items-center gap-3">
                <StatusDot status="live" />
                <StatusDot status="devnet" />
                <StatusDot status="pending" />
                <StatusDot status="planned" />
              </span>
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection title="Card" description="The basic surface. Header, title, description, optional action, content and footer.">
          <ThemePair>
            <Card>
              <CardHeader>
                <CardTitle>Card title</CardTitle>
                <CardDescription>A short description of what the card holds.</CardDescription>
                <CardAction>
                  <StatusBadge status="devnet">Devnet</StatusBadge>
                </CardAction>
              </CardHeader>
              <CardContent className="text-sm text-muted-foreground">
                Content area. Cards sit on the page background with a border and a hairline highlight.
              </CardContent>
              <CardFooter className="gap-3 border-t">
                <Button size="sm">Action</Button>
                <Button size="sm" variant="ghost">
                  Cancel
                </Button>
              </CardFooter>
            </Card>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Tabs"
          description="Radix tabs: arrow keys move between triggers, Tab moves into the panel. The first row is interactive; the second forces each trigger state."
        >
          <ThemePair>
            <Tabs defaultValue="web">
              <TabsList aria-label="Example tabs">
                <TabsTrigger value="web">Web node</TabsTrigger>
                <TabsTrigger value="native">Native node</TabsTrigger>
                <TabsTrigger value="archive" disabled>
                  Archive
                </TabsTrigger>
              </TabsList>
              <TabsContent value="web" className="text-sm text-muted-foreground">
                Panel for the first tab.
              </TabsContent>
              <TabsContent value="native" className="text-sm text-muted-foreground">
                Panel for the second tab.
              </TabsContent>
            </Tabs>
            <Specimen label="inactive · hover · focus · active · disabled">
              <TabStates />
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Table"
          description="For comparisons and node data. Row headers use th scope=&quot;row&quot;; numbers are mono and right-aligned. A table that can overflow gets a focusable, labelled scroll container."
        >
          <ThemePair layout="stack">
            <ExampleTable>
              <TableCaption>Caption: what the table compares, and any caveat.</TableCaption>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Check</TableHead>
                  <TableHead scope="col">Compared</TableHead>
                  <TableHead scope="col" className="text-right">
                    Result
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {[
                  ["Rest", "", "12 / 12"],
                  ["Hover", "hover", "8 / 8"],
                  ["Selected", "selected", "3 / 3"],
                ].map(([label, state, result]) => (
                  <TableRow
                    key={label}
                    data-preview={state === "hover" ? "hover" : undefined}
                    data-state={state === "selected" ? "selected" : undefined}
                  >
                    <TableCell className="font-medium">{label} row</TableCell>
                    <TableCell className="text-muted-foreground">Description of what was compared</TableCell>
                    <TableCell className="text-right font-mono text-brand-text tabular-nums">{result}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </ExampleTable>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Alert"
          description="A static note (role=&quot;note&quot;), not a live announcement. Use caution for limits and caveats, destructive for errors."
        >
          <ThemePair>
            <div className="flex flex-col gap-3">
              <Alert>
                <Info />
                <AlertTitle>Default</AlertTitle>
                <AlertDescription>Neutral information.</AlertDescription>
              </Alert>
              <Alert variant="brand">
                <Info />
                <AlertTitle>Brand</AlertTitle>
                <AlertDescription>Something worth pointing out.</AlertDescription>
              </Alert>
              <Alert variant="caution">
                <CircleSlash />
                <AlertTitle>Caution</AlertTitle>
                <AlertDescription>A limit or caveat the reader should know.</AlertDescription>
              </Alert>
              <Alert variant="destructive">
                <TriangleAlert />
                <AlertTitle>Destructive</AlertTitle>
                <AlertDescription>Something failed or needs action.</AlertDescription>
              </Alert>
            </div>
          </ThemePair>
        </Subsection>

        <Subsection title="Separator" description="Decorative by default (hidden from assistive technology).">
          <ThemePair>
            <div className="flex flex-col gap-4">
              <Separator />
              <div className="flex h-8 items-center gap-4 text-sm text-muted-foreground">
                One
                <Separator orientation="vertical" />
                Two
                <Separator orientation="vertical" />
                Three
              </div>
            </div>
          </ThemePair>
        </Subsection>
        <Subsection
          title="Input, textarea and label"
          description="Text fields for the dashboard's forms; Textarea is Input over several lines. The border is --field, 3:1 against page and card. Pair every input with a Label; aria-invalid marks a field in error, with the message in text next to it (aria-describedby)."
        >
          <ThemePair>
            <div className="flex flex-wrap gap-4">
              <Field label="Rest" placeholder="B62q…" className="font-mono" />
              <Field label="Focus" defaultValue="0.1" data-preview="focus" />
              <Field label="Invalid" defaultValue="B62short" aria-invalid className="font-mono" />
              <Field label="Disabled" defaultValue="Read only" disabled />
            </div>
            <div className="flex flex-wrap gap-4">
              <TextareaField label="What happened?" placeholder="A sentence or two" />
              <TextareaField label="Focus" defaultValue="Stops syncing after an hour." data-preview="focus" />
            </div>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Skeleton"
          description="Placeholders while the first answer loads, in the shape of what is coming. Decorative: put the loading text in an sr-only role=status next to them. The pulse stops under reduced motion."
        >
          <ThemePair>
            <div className="flex flex-col gap-3 rounded-xl border bg-card p-5">
              <Skeleton className="h-8 w-48" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </div>
          </ThemePair>
        </Subsection>
      </ShowcaseSection>

      <ShowcaseSection
        id="patterns"
        title="Patterns"
        description="Brand compositions built from the tokens, in src/patterns. The node dashboard (ui/dashboard) builds its pages from these."
      >
        <Subsection
          title="Logo"
          description="Logo A: brand teal on a white tile, rounded by default, square for places that crop their own corners. Keep the white tile on both themes. Minimum size 24 px."
        >
          <ThemePair>
            <Specimen label="mark · 24, 32, 48, 64 px · square">
              <LogoMark className="size-6" />
              <LogoMark className="size-8" />
              <LogoMark className="size-12" />
              <LogoMark className="size-16" title="minanode" />
              <LogoMark shape="square" className="size-12" />
            </Specimen>
            <Specimen label="mark and wordmark">
              <Logo />
              <Logo className="text-2xl" markClassName="size-11" />
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection title="Eyebrow and section heading" description="Mono eyebrow, title, lead. Titles are h2 by default.">
          <ThemePair>
            <Eyebrow>Eyebrow label</Eyebrow>
            <SectionHeading
              as="h3"
              eyebrow="Section"
              title="Section title"
              description="A lead paragraph that sets up the section in a sentence or two."
            />
          </ThemePair>
        </Subsection>

        <Subsection
          title="Metric card"
          description="A number, its label and its caveat. The caveat is not optional on the site: a number without its bounds is not shown."
        >
          <ThemePair>
            <div className="grid gap-3 sm:grid-cols-2">
              <MetricCard value="1,234" label="Default size" detail="The caveat that has to travel with the number." />
              <MetricCard
                size="sm"
                value="56.7 s"
                label="Small size"
                detail="For grids inside panels."
                footer={<ExternalLink href="#patterns">Source link</ExternalLink>}
              />
            </div>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Stat"
          description="A number, label and caveat without a card, for rows of figures inside a panel. The default variant puts the value first in teal; it shrinks to fit and never wraps. The labelled variant puts a mono label first and a sentence of detail under the value, for an app's live figures; with inList its parts are dt and dd inside a dl."
        >
          <ThemePair>
            <Specimen label="default">
              <div className="grid w-full gap-6 border-t pt-6 sm:grid-cols-3">
                <Stat value="400 s" label="Short value" detail="A short caveat." />
                <Stat value="16.2–17.3 s" label="Range value" detail="Fits a narrow column." />
                <Stat value="1,234 MiB" label="Without detail" />
              </div>
            </Specimen>
            <Specimen label="labelled · inList">
              <dl className="grid w-full grid-cols-2 rounded-xl border [&>div:first-child]:border-r">
                <Stat variant="labelled" inList className="px-4 py-5" label="Best tip" value="578,883" detail="block 45 s old" />
                <Stat variant="labelled" inList className="px-4 py-5" label="Peers" value="12" detail="connected; 28 more known" />
              </dl>
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection title="Key-value receipt" description="A description list in mono for receipts and node details.">
          <ThemePair>
            <KeyValueList
              title="Receipt title"
              aside={
                <span className="inline-flex items-center gap-2 text-xs text-brand-text">
                  <StatusDot status="live" /> scope
                </span>
              }
              rows={[
                { label: "Field", value: "value" },
                { label: "Highlighted", value: "brand value", tone: "brand" },
                { label: "Long value", value: "3NKiFwqrZkMm4EUR9Ud1VJzWDoDL1uRgEYVADVigKm6adyhNW7SR" },
              ]}
              footer="A footnote with the caveat for these values."
            />
          </ThemePair>
        </Subsection>

        <Subsection
          title="External link and theme toggle"
          description="External links open a new tab and say so to screen readers. The theme toggle switches the whole page and remembers the choice."
        >
          <ThemePair>
            <Specimen label="external link · rest, hover, focus">
              <ExternalLink href="#patterns">Rest</ExternalLink>
              <ExternalLink href="#patterns" data-preview="hover">
                Hover
              </ExternalLink>
              <ExternalLink href="#patterns" data-preview="focus">
                Focus
              </ExternalLink>
            </Specimen>
            <Specimen label="theme toggle (switches the page)">
              <ThemeToggle />
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Signal"
          description="The live state of something running: a node's sync, a worker, a peer. ok is working, info is in progress with nothing wrong, warn needs a look, error is not working, neutral is off or unknown. Always with words; pulse only for something in progress. StatusBadge stays for evidence status on the site."
        >
          <ThemePair>
            <Specimen label="tones">
              <Signal tone="ok">Synced</Signal>
              <Signal tone="info">Catching up</Signal>
              <Signal tone="warn">Not synced</Signal>
              <Signal tone="error">No answer</Signal>
              <Signal tone="neutral">Off</Signal>
            </Specimen>
            <Specimen label="pulse · with a figure">
              <Signal tone="info" pulse>
                Bootstrapping <span className="text-foreground/80">to #578890</span>
              </Signal>
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="Hash"
          description="Hashes, keys and peer IDs in mono: the ends when truncated, the whole value in the title and to screen readers, and a copy button named for what it copies."
        >
          <ThemePair>
            <Specimen label="truncated · whole · without copy">
              <Hash value="B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t" label="public key" />
              <Hash value="3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W" truncate={false} label="state hash" />
              <Hash value="2aTKv6vapFoRUoKea9FTD3vxJm5qiYBMJcD2mGyjgdcryXGHHFt" copy={false} />
            </Specimen>
          </ThemePair>
        </Subsection>

        <Subsection
          title="JSON tree"
          description="Any JSON value as a collapsible tree: objects and arrays fold, the first two levels start open (openDepth), long arrays show pageSize items at a time, and Copy JSON copies the whole value pretty-printed. Folds keep their state when the value refreshes. It knows nothing about where the value came from."
        >
          <ThemePair>
            <JsonTreeSample />
          </ThemePair>
        </Subsection>

        <Subsection
          title="Code block"
          description="A command or snippet to copy, in mono. It scrolls sideways inside a focusable, named region, and its copy button is named for what it copies."
        >
          <ThemePair>
            <CodeBlockSample />
          </ThemePair>
        </Subsection>

        <Subsection
          title="Empty state"
          description="What a panel says when it has nothing to show: no data yet, a feature that is off, a source that does not answer. A title, what to do, and an action when there is one."
        >
          <ThemePair layout="stack">
            <EmptyState
              icon={<Lock />}
              title="Control is off on this node"
              description={<p>Restart the node with the flag that turns it on.</p>}
            />
            <EmptyState
              tone="destructive"
              icon={<Unplug />}
              title="No answer from the node"
              description={<p>The node isn't answering.</p>}
              action={
                <Button variant="outline" size="sm">
                  Try again
                </Button>
              }
            />
          </ThemePair>
        </Subsection>
      </ShowcaseSection>
    </>
  );
}
