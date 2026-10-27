import * as React from "react";
import { Pause, Play } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { JsonTree } from "@minanode/design-system/patterns/json-tree";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { SegmentedControl } from "@minanode/design-system/patterns/segmented-control";
import { STATUS_POLL_MS, type RawRoute } from "@minanode/node-client";
import { useNodeClient, useRaw } from "@minanode/node-client/react";

import { Loaded, PolledAge } from "../components/states";

/** The node's answers this page shows: read-only GETs the dashboard already
 * reads, nothing under `/state`. */
export const RAW_SOURCES = [
  {
    path: "/status",
    label: "Node status",
    about: "Sync, best tip and root, peers, pools, roles and resources.",
  },
  {
    path: "/build_env",
    label: "Build",
    about: "Version, commit, cargo features and the compiler.",
  },
] as const satisfies readonly {
  path: RawRoute;
  label: string;
  about: string;
}[];

export function NodeStatusPage() {
  const client = useNodeClient();
  const [path, setPath] = React.useState<RawRoute>(RAW_SOURCES[0].path);
  const source = RAW_SOURCES.find(item => item.path === path) ?? RAW_SOURCES[0];
  const [paused, setPaused] = React.useState(false);
  const raw = useRaw(source.path, { paused, intervalMs: STATUS_POLL_MS });

  return (
    <>
      <PageHeader
        eyebrow="Node"
        title="Status"
        description={
          <>
            The node&apos;s own answer to{" "}
            <code className="font-mono text-foreground">GET {source.path}</code>
            , as it sends it. {source.about}
          </>
        }
        aside={
          <>
            <PolledAge
              polled={raw}
              staleAfterMs={STATUS_POLL_MS * 4}
              paused={paused}
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              aria-pressed={paused}
              onClick={() => setPaused(p => !p)}
            >
              {paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {paused ? "Resume" : "Pause"}
            </Button>
          </>
        }
      />
      <div className="flex flex-wrap items-center justify-between gap-3">
        <SegmentedControl
          label="Which answer"
          options={RAW_SOURCES.map(item => ({
            value: item.path,
            label: item.path,
          }))}
          value={source.path}
          onValueChange={setPath}
        />
        <ExternalLink href={client.url(source.path)} className="text-sm">
          Open raw {source.path}
        </ExternalLink>
      </div>
      <Loaded polled={raw} what={source.path} rows={8}>
        {value => (
          <JsonTree key={source.path} value={value} label={source.label} />
        )}
      </Loaded>
    </>
  );
}
