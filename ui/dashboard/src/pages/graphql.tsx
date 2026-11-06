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
        eyebrow="Node"
        title="GraphQL"
        aside={
          // The one link to the explanation.
          <ExternalLink href={docsUrl("graphql")} className="text-sm">
            GraphQL API docs
          </ExternalLink>
        }
      />
      <KeyValueList
        title="Endpoint"
        align="left"
        rows={[
          {
            label: "URL",
            value: (
              <Hash
                value={endpoint}
                truncate={false}
                label="GraphQL endpoint"
              />
            ),
            tone: "brand",
          },
          { label: "Method", value: "POST" },
          { label: "Body", value: '{"query": "…", "variables": {…}}' },
        ]}
        footer="Browsers on https can't reach plain http: use curl or a local client."
      />
      <PageSection title="Try it">
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <p className="text-sm font-medium text-foreground">Query</p>
            <CodeBlock code={EXAMPLE_QUERY} label="example query" />
          </div>
          <div className="flex flex-col gap-1.5">
            <p className="text-sm font-medium text-foreground">curl</p>
            <CodeBlock
              code={curlCommand(endpoint, EXAMPLE_QUERY)}
              label="curl command"
              wrap={false}
            />
          </div>
        </div>
      </PageSection>
    </>
  );
}
