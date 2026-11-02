import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocSection, Prose } from "@/components/docs/prose";
import { bugReports } from "@/content/docs";

export const metadata: Metadata = {
  title: "Report a bug",
  description:
    "Report a minanode bug with mina report or the dashboard's Report a problem page: what a report holds, and what it leaves out.",
  alternates: { canonical: "/docs/bug-reports/" },
};

const journal = `journalctl --user -u minanode.service --since "1 hour ago" | mina report --log -`;

export default function BugReportsPage() {
  return (
    <DocsPage
      slug="bug-reports"
      title="Report a bug"
      lead="If the node crashes, stops syncing, or misbehaves, one command collects what the developers need."
    >
      <DocSection id="mina-report" title="Run mina report">
        <CodeBlock code="mina report" label="report command" />
        <Prose>
          <p>
            <C>mina report</C> asks what happened, collects the report, and
            prints it. In an interactive terminal, after you confirm, it opens a
            pre-filled GitHub issue in your browser. In a noninteractive shell,
            it prints the report and stops, unless you pass <C>--yes</C>.
            Nothing is posted until you submit the issue yourself.
          </p>
          <p>
            The command reads the node&apos;s work directory, which is{" "}
            <C>~/.mina</C> unless you give <C>-d DIR</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="what-it-holds" title="What a report holds">
        <Prose>
          <p>A report holds these parts:</p>
          <ul>
            <li>this build: its version, commit, and build flags</li>
            <li>the machine: OS, CPU, RAM, and free disk</li>
            <li>
              what the running node says about itself: sync state, tips, peers,
              and roles
            </li>
            <li>the newest crash, if there was one</li>
            <li>warnings and errors from the end of the log</li>
            <li>a check of the saved chain state</li>
            <li>how the node was started</li>
          </ul>
          <p>
            A report never holds private keys or key files, passwords, the
            values of secret environment variables, tokens, or anything in a web
            address after its host. It leaves out peer IP addresses and peer ids
            unless you add <C>--include-network-ids</C>.
          </p>
          <p>{bugReports.sharingNote}</p>
        </Prose>
      </DocSection>

      <DocSection id="options" title="Options">
        <Prose>
          <p>These options change what the command does:</p>
          <ul>
            <li>
              <C>-m &quot;stops syncing after an hour&quot;</C> says what
              happened, so the command does not ask.
            </li>
            <li>
              <C>--no-browser</C> prints the issue link instead of opening it.
              Use it on a server, and open the link on any machine. Without a
              terminal to confirm on, also add <C>--yes</C>. Otherwise the
              command prints no link, and only saves the report in{" "}
              <C>bug-reports/</C>.
            </li>
            <li>
              <C>--output report.md</C> only saves the report, and prints its
              full path.
            </li>
            <li>
              <C>--log</C> reads the log from elsewhere, for a node that logs to
              the journal:
            </li>
          </ul>
        </Prose>
        <CodeBlock code={journal} label="report from the journal" />
        <Prose>
          <p>
            If a report is too long for a link, the command saves it in{" "}
            <C>bug-reports/</C> in the work directory. Attach that file to the
            issue.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="from-the-dashboard" title="Report from the dashboard">
        <Prose>
          <p>
            The dashboard&apos;s Report a problem page makes the same report.
            Say what happened and press <strong>Prepare report</strong>. Then
            copy the report, download it, or open the GitHub issue. The page
            collects nothing until you press the button. Nothing leaves the page
            until you copy, download, or open the report.
          </p>
          <p>
            The dashboard&apos;s report skips the check of the saved chain
            state. For a node that logs to the journal, use{" "}
            <C>mina report --log -</C> as above.
          </p>
          <p>
            After a recent crash, or a restart after one, the dashboard&apos;s
            Overview page says when the crash happened. It shows the first line
            of the panic and a link to the Report a problem page.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
