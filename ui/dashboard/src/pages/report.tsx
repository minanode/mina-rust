import * as React from "react";
import {
  Download,
  ExternalLink as ExternalIcon,
  FileText,
  TriangleAlert,
} from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Checkbox } from "@minanode/design-system/components/checkbox";
import { Label } from "@minanode/design-system/components/label";
import { Textarea } from "@minanode/design-system/components/textarea";
import { CopyButton } from "@minanode/design-system/patterns/copy-button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TimeAgo } from "@minanode/design-system/patterns/relative-time";
import {
  describeError,
  kindOf,
  reportDocument,
  type Report,
} from "@minanode/node-client";
import { useNodeClient } from "@minanode/node-client/react";

import { Code } from "../components/code";
import { docsUrl } from "../lib/docs";

type State =
  | { kind: "idle" }
  | { kind: "preparing" }
  | { kind: "ready"; report: Report }
  | { kind: "failed"; error: unknown };

export function ReportPage() {
  const client = useNodeClient();
  const [description, setDescription] = React.useState("");
  const [includeNetworkIds, setIncludeNetworkIds] = React.useState(false);
  const [state, setState] = React.useState<State>({ kind: "idle" });
  const descriptionId = React.useId();
  const idsId = React.useId();
  const controller = React.useRef<AbortController | null>(null);
  React.useEffect(() => () => controller.current?.abort(), []);

  // A prepared report shows what was asked: changing the options makes it
  // out of date, so it goes.
  const change =
    <T,>(set: (value: T) => void) =>
    (value: T) => {
      set(value);
      setState(s =>
        s.kind === "ready" || s.kind === "failed" ? { kind: "idle" } : s,
      );
    };

  async function prepare() {
    controller.current?.abort();
    const abort = new AbortController();
    controller.current = abort;
    setState({ kind: "preparing" });
    try {
      const report = await client.report(
        { includeNetworkIds, description },
        abort.signal,
      );
      if (!abort.signal.aborted) setState({ kind: "ready", report });
    } catch (error) {
      if (!abort.signal.aborted) setState({ kind: "failed", error });
    }
  }

  const preparing = state.kind === "preparing";

  return (
    <>
      <PageHeader
        eyebrow="Help"
        title="Report a problem"
        aside={
          // The one link to the explanation.
          <ExternalLink href={docsUrl("bugReports")} className="text-sm">
            What a report holds
          </ExternalLink>
        }
      />

      <Card>
        <CardHeader>
          <CardTitle>Prepare a report</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <CardDescription>
            No keys, passwords or tokens. Nothing leaves this page until you
            copy, download or open it.
          </CardDescription>
          <form
            className="flex flex-col gap-4"
            onSubmit={event => {
              event.preventDefault();
              void prepare();
            }}
          >
            <div className="flex flex-col gap-2">
              <Label htmlFor={descriptionId}>
                What happened?{" "}
                <span className="font-normal text-muted-foreground">
                  Optional
                </span>
              </Label>
              <Textarea
                id={descriptionId}
                className="max-w-3xl"
                value={description}
                onChange={event => change(setDescription)(event.target.value)}
                maxLength={2000}
                rows={3}
              />
            </div>
            <div className="flex items-start gap-3">
              <Checkbox
                id={idsId}
                checked={includeNetworkIds}
                onChange={event =>
                  change(setIncludeNetworkIds)(event.target.checked)
                }
              />
              <Label htmlFor={idsId} className="leading-5 text-pretty">
                Keep IP addresses and peer IDs in the report
              </Label>
            </div>
            <div className="flex flex-wrap items-center gap-3 pt-1">
              {/* Once a report is ready, opening its issue is the page's
                  one filled action. */}
              <Button
                type="submit"
                variant={state.kind === "ready" ? "outline" : "default"}
                disabled={preparing}
                aria-busy={preparing}
              >
                <FileText aria-hidden />
                {preparing
                  ? "Preparing…"
                  : state.kind === "ready"
                    ? "Prepare again"
                    : "Prepare report"}
              </Button>
              <p
                role="status"
                className="text-sm text-muted-foreground empty:hidden"
              >
                {preparing ? "Collecting the report from the node…" : ""}
              </p>
            </div>
          </form>
        </CardContent>
        <CardFooter className="text-sm text-muted-foreground">
          <p className="text-pretty">
            For an AI-drafted issue, run <Code>mina triage</Code> on the
            node&apos;s machine.
          </p>
        </CardFooter>
      </Card>

      {state.kind === "failed" ? <Failed error={state.error} /> : null}
      {state.kind === "ready" ? <Prepared report={state.report} /> : null}
    </>
  );
}

function Failed({ error }: { error: unknown }) {
  const kind = kindOf(error);
  return (
    <Alert variant="destructive">
      <TriangleAlert aria-hidden />
      <AlertTitle>No report</AlertTitle>
      <AlertDescription>
        <p>
          {kind === "absent"
            ? "This node doesn't prepare reports: it is older than this dashboard."
            : describeError(error)}
        </p>
        <p>
          Run <Code>mina report</Code> on the node&apos;s machine instead.
        </p>
      </AlertDescription>
    </Alert>
  );
}

function download(report: Report) {
  const url = URL.createObjectURL(
    new Blob([reportDocument(report)], { type: "text/markdown;charset=utf-8" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = report.fileName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Prepared({ report }: { report: Report }) {
  const regionId = React.useId();
  return (
    <section aria-labelledby={regionId} className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <p className="font-mono text-label text-muted-foreground">
          <span className="uppercase">Prepared</span>{" "}
          {report.generatedMs !== null ? (
            <TimeAgo at={report.generatedMs} />
          ) : null}
          {report.includeNetworkIds ? ", with network IDs" : null}
        </p>
        <h2 id={regionId} className="text-heading text-balance text-foreground">
          {report.title}
        </h2>
      </div>
      {!report.issue.complete ? (
        <Alert variant="caution">
          <TriangleAlert aria-hidden />
          <AlertTitle>Too long for an issue link</AlertTitle>
          <AlertDescription>
            <p>
              The link holds the summary only. Attach the downloaded file, or
              paste the report, before you submit.
            </p>
          </AlertDescription>
        </Alert>
      ) : null}
      <div className="flex flex-col gap-2">
        {/* On a phone the filled action comes first, at full width, the
            other two side by side under it. The page's outcome: buttons at
            full size. */}
        <div className="flex flex-wrap items-center gap-2">
          <CopyButton
            variant="text"
            text={report.markdown}
            label="the report"
            title="Copy the report as Markdown"
            className="h-10 border border-input px-4 text-sm max-sm:flex-1"
          >
            Copy
          </CopyButton>
          {/* Too long for the issue link, the file is the step that
              matters: Download is the filled action then. */}
          <Button
            type="button"
            variant={report.issue.complete ? "outline" : "default"}
            className={
              report.issue.complete
                ? "max-sm:flex-1"
                : "max-sm:order-first max-sm:w-full"
            }
            onClick={() => download(report)}
          >
            <Download aria-hidden />
            Download (.md)
          </Button>
          <Button
            asChild
            variant={report.issue.complete ? "default" : "outline"}
            className={
              report.issue.complete
                ? "max-sm:order-first max-sm:w-full"
                : "max-sm:flex-1"
            }
          >
            <a
              href={report.issue.url}
              target="_blank"
              rel="noopener noreferrer"
            >
              <ExternalIcon aria-hidden />
              Open GitHub issue
              <span className="sr-only"> (opens in a new tab)</span>
            </a>
          </Button>
        </div>
        <p className="text-sm text-pretty text-muted-foreground">
          The issue opens on github.com/{report.issue.repo}. Nothing is posted
          until you submit it there.
        </p>
      </div>
      <pre
        role="region"
        aria-label="The report"
        tabIndex={0}
        className="max-h-[36rem] overflow-auto rounded-xl border bg-card p-4 font-mono text-[0.8125rem] leading-relaxed break-words whitespace-pre-wrap text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        {report.markdown}
      </pre>
    </section>
  );
}
