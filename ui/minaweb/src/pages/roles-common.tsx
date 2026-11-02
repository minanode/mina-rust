/** Parts the key, send, SNARK worker and block producer screens share. */

import * as React from "react";
import { KeyRound, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { Link } from "../app/router";
import { CONFIG } from "../config";
import { formatBytes } from "../boot/failures";
import {
  deviceMemoryGb,
  lowMemoryWarning,
  ROLE_COSTS,
  SWITCH_PEAK_BYTES,
} from "../roles/resources";
import { rolesAllowed } from "../roles/role";

/** The devnet faucet: a link only, the page never calls it. */
export const FAUCET_URL = "https://faucet.minaprotocol.com/";

/** The plain statement every key and role screen makes. */
export function DevnetOnly() {
  return (
    <p className="text-sm text-pretty text-muted-foreground">
      Devnet only: this page has no mainnet mode. Devnet MINA has no value.
    </p>
  );
}

/** On a build for another network, the role screens say so and stop. */
export function RolesGate({ children }: { children: React.ReactNode }) {
  if (rolesAllowed(CONFIG.network)) return <>{children}</>;
  return (
    <EmptyState
      icon={<KeyRound />}
      title="Keys and roles are devnet only"
      description={`This page runs a ${CONFIG.network} node, which follows the chain only. The devnet page has keys, payments, the SNARK worker and the block producer.`}
    />
  );
}

export function NeedKey({ what }: { what: string }) {
  return (
    <EmptyState
      icon={<KeyRound />}
      title="No key in this tab"
      description={`${what} needs a devnet key. Make one, or import a devnet key, on the key screen.`}
      action={
        <Button asChild>
          <Link to="/key">Go to the key screen</Link>
        </Button>
      }
    />
  );
}

const GIB = 1024 ** 3;

/** What a role costs this device, before Start. */
export function RoleCost({ kind }: { kind: "worker" | "producer" }) {
  const cost = ROLE_COSTS[kind];
  const reported = deviceMemoryGb();
  const warning = lowMemoryWarning(kind, reported);
  return (
    <div className="flex flex-col gap-4">
      <KeyValueList
        title="What it takes"
        rows={[
          {
            label: "Prover circuits to download",
            value: formatBytes(cost.circuitBytes),
          },
          {
            label: "Tab memory at its peak",
            value: `about ${(cost.peakBytes / GIB).toFixed(1)} GiB, ${cost.peakBasis}`,
          },
          {
            label: "Prover ready after",
            value: `about ${cost.setupSeconds} s`,
          },
          {
            label: "This device reports",
            value:
              reported === null
                ? "nothing (only Chromium browsers say)"
                : `${reported >= 8 ? "8 GB or more" : `${reported} GB`} of memory`,
          },
        ]}
        footer={`Measured in headless Chrome on devnet, node and prover together. Starting or stopping a role starts the node again: for about 30 s the tab also holds the old node's memory, up to about ${(SWITCH_PEAK_BYTES / GIB).toFixed(1)} GiB. The tab must stay open: closing it stops the role.`}
      />
      {warning ? (
        <Alert variant="caution" data-low-memory>
          <TriangleAlert aria-hidden />
          <AlertTitle>This device may be short of memory</AlertTitle>
          <AlertDescription>
            <p>{warning}</p>
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}

/** The one-role rule, said the same way on both role screens. */
export const ONE_ROLE =
  "A web node runs the SNARK worker or the block producer, not both: a SNARK proof can't be stopped for a block proof, which could then miss its slot. Choosing one stops the other.";
