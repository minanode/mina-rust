import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocSection, Prose } from "@/components/docs/prose";
import { bugReports } from "@/content/docs";

export const metadata: Metadata = {
  title: "Reporting a bug",
  description:
    "Report a minanode bug with mina report or the dashboard's Report a problem page: what a report holds, and what it leaves out.",
  alternates: { canonical: "/docs/bug-reports/" },
};

const journal = `journalctl --user -u minanode.service --since "1 hour ago" | mina report --log -`;

export default function BugReportsPage() {
  return (
    <DocsPage
      slug="bug-reports"
      title="Reporting a bug"
      lead="If the node crashes, stops syncing or misbehaves, one command collects what the developers need."
    >
      <DocSection id="mina-report" title="mina report">
        <CodeBlock code="mina report" label="report command" />
        <Prose>
          <p>
            <C>mina report</C> asks what happened, collects the report and
            prints it. Once you confirm in an interactive terminal, it opens a
            pre-filled GitHub issue in your browser. In a noninteractive shell,
            it prints the report and stops unless you pass <C>--yes</C>. Nothing
            is posted until you submit the issue yourself.
          </p>
          <p>
            It reads the node&apos;s work directory, <C>~/.mina</C> unless you
            give <C>-d DIR</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="what-it-holds" title="What a report holds">
        <Prose>
          <ul>
            <li>this build: its version, commit and build flags;</li>
            <li>the machine: OS, CPU, RAM and free disk;</li>
            <li>
              what the running node says about itself: sync state, tips, peers
              and roles;
            </li>
            <li>the newest crash, if there was one;</li>
            <li>warnings and errors from the end of the log;</li>
            <li>a check of the saved chain state;</li>
            <li>how the node was started.</li>
          </ul>
          <p>
            It never holds private keys or key files, passwords or the values of
            secret environment variables, tokens, or anything in a web address
            after its host. Peer IP addresses and peer ids are left out unless
            you add <C>--include-network-ids</C>.
          </p>
          <p>{bugReports.sharingNote}</p>
        </Prose>
      </DocSection>

      <DocSection id="options" title="Options">
        <Prose>
          <ul>
            <li>
              <C>-m &quot;stops syncing after an hour&quot;</C> says what
              happened without being asked.
            </li>
            <li>
              <C>--no-browser</C> prints the issue link instead of opening it,
              for a server: open the link on any machine. Without a terminal to
              confirm on, add <C>--yes</C>, or no link is printed and the report
              is only saved in <C>bug-reports/</C>.
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
            A report too long for a link is saved in <C>bug-reports/</C> in the
            work directory; attach that file to the issue.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="from-the-dashboard" title="From the dashboard">
        <Prose>
          <p>
            The dashboard&apos;s Report a problem page makes the same report.
            Say what happened, press Prepare report, then copy it, download it
            or open the GitHub issue. Nothing is collected until you press the
            button, and nothing leaves the page until you copy, download or open
            it.
          </p>
          <p>
            The dashboard&apos;s report skips the check of the saved chain
            state. For a node that logs to the journal, use{" "}
            <C>mina report --log -</C> as above.
          </p>
          <p>
            After a recent crash, or a restart after one, the dashboard&apos;s
            overview says when it happened, with the first line of the panic and
            a link to the Report a problem page.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
