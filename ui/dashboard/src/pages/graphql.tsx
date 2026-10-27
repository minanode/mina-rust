import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { useNodeClient } from "@minanode/node-client/react";

import { curlCommand, docsUrl, EXAMPLE_QUERY } from "../lib/docs";

export function GraphqlPage() {
  const client = useNodeClient();
  const endpoint = new URL(client.url("/graphql"), window.location.href).href;

  return (
    <>
      <PageHeader
        eyebrow="API"
        title="GraphQL"
        description="The node answers the same GraphQL API as the OCaml node, at its own address, for wallets, explorers and scripts."
      />
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <KeyValueList
          title="Endpoint"
          rows={[
            {
              label: "URL",
              value: (
                <Hash
                  value={endpoint}
                  truncate={false}
                  label="GraphQL endpoint"
                  className="sm:justify-end"
                />
              ),
              tone: "brand",
            },
            { label: "Method", value: "POST" },
            { label: "Body", value: '{"query": "…", "variables": {…}}' },
          ]}
        />
        <div className="flex flex-col gap-3 rounded-xl border bg-card p-6 text-sm leading-relaxed text-muted-foreground">
          <p className="font-semibold text-foreground">Explore it</p>
          <p>
            Use any GraphQL client on your computer, pointed at the URL: a
            desktop app, a library, or curl as below. A web explorer served over
            https can&apos;t reach a node served over plain http (the browser
            blocks mixed content), so use one that runs locally.
          </p>
          <p>
            The queries, mutations and limits are in the docs:{" "}
            <ExternalLink href={docsUrl("graphql")}>GraphQL API</ExternalLink>{" "}
            and <ExternalLink href={docsUrl("http")}>HTTP API</ExternalLink>.
          </p>
        </div>
      </div>
      <PageSection
        title="Try it"
        description="The sync phase and the best tip's hash and height."
      >
        <div className="flex flex-col gap-3">
          <CodeBlock code={EXAMPLE_QUERY} label="example query" />
          <CodeBlock
            code={curlCommand(endpoint, EXAMPLE_QUERY)}
            label="curl command"
          />
        </div>
      </PageSection>
    </>
  );
}
