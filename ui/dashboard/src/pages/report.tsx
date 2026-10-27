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
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
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
  const hintId = React.useId();
  const idsId = React.useId();
  const idsHintId = React.useId();
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
        eyebrow="Support"
        title="Report a problem"
        description={
          <>
            A report tells others enough about this node to help: the build, the
            machine, how the node was started, what it says about itself, and
            its newest crash, warnings and errors. It never holds keys,
            passwords, secret environment variables or tokens, and leaves out IP
            addresses and peer ids unless you ask.{" "}
            <ExternalLink href={docsUrl("bugReports")}>
              What a report holds
            </ExternalLink>
          </>
        }
      />

      <Card className="gap-5 lg:max-w-3xl">
        <CardHeader>
          <CardTitle>Prepare a report</CardTitle>
          <CardDescription>
            Nothing is collected until you press Prepare report, and nothing
            leaves the page until you copy, download or open it. Unlike{" "}
            <Code>mina report</Code>, it skips the check of the saved chain
            state.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="flex flex-col gap-4"
            onSubmit={event => {
              event.preventDefault();
              void prepare();
            }}
          >
            <div className="flex flex-col gap-2">
              <Label htmlFor={descriptionId}>What happened?</Label>
              <Textarea
                id={descriptionId}
                value={description}
                onChange={event => change(setDescription)(event.target.value)}
                aria-describedby={hintId}
                maxLength={2000}
                rows={3}
              />
              <p id={hintId} className="text-sm text-muted-foreground">
                A sentence or two; you may leave it empty.
              </p>
            </div>
            <div className="flex items-start gap-3">
              <input
                id={idsId}
                type="checkbox"
                checked={includeNetworkIds}
                onChange={event =>
                  change(setIncludeNetworkIds)(event.target.checked)
                }
                aria-describedby={idsHintId}
                className="mt-0.5 size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
              />
              <div className="flex flex-col gap-1">
                <Label htmlFor={idsId}>
                  Keep IP addresses and peer ids in the report
                </Label>
                <p id={idsHintId} className="text-sm text-muted-foreground">
                  Left out by default.
                </p>
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-3 pt-1">
              <Button type="submit" disabled={preparing} aria-busy={preparing}>
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
          <p className="mt-5 text-sm text-muted-foreground">
            To have an AI agent draft the issue from this same report, run{" "}
            <Code>mina triage</Code> on the node&apos;s machine: it stays on the
            command line.
          </p>
        </CardContent>
      </Card>

      {state.kind === "failed" ? <Failed error={state.error} /> : null}
      {state.kind === "ready" ? <Prepared report={state.report} /> : null}
    </>
  );
}

function Failed({ error }: { error: unknown }) {
  const kind = kindOf(error);
  return (
    <Alert variant="destructive" className="lg:max-w-3xl">
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
    <section
      aria-labelledby={regionId}
      className="flex flex-col gap-4 lg:max-w-5xl"
    >
      <div className="flex flex-col gap-1">
        <p className="font-mono text-label text-muted-foreground uppercase">
          Prepared{" "}
          {report.generatedMs !== null ? (
            <TimeAgo at={report.generatedMs} />
          ) : null}
          {report.includeNetworkIds ? ", with network ids" : null}
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
              The issue opens with the report&apos;s summary. Download the
              report and attach the file to the issue, or paste it, before you
              submit.
            </p>
          </AlertDescription>
        </Alert>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <CopyButton
          variant="text"
          text={report.markdown}
          label="the report"
          title="Copy the report as Markdown"
          className="border border-input"
        >
          Copy
        </CopyButton>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => download(report)}
        >
          <Download aria-hidden />
          Download (.md)
        </Button>
        <Button asChild size="sm">
          <a href={report.issue.url} target="_blank" rel="noopener noreferrer">
            <ExternalIcon aria-hidden />
            Open GitHub issue
            <span className="sr-only"> (opens in a new tab)</span>
          </a>
        </Button>
        <span className="text-sm text-muted-foreground">
          on github.com/{report.issue.repo}: nothing is posted until you submit
          it there.
        </span>
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
