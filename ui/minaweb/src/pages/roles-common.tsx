/** Parts the key, send, SNARK worker and block producer screens share. */

import * as React from "react";
import { ArrowRight, Info, KeyRound, TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { Button } from "@minanode/design-system/components/button";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { Measure } from "@minanode/design-system/patterns/amount";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { Link } from "../app/router";
import { CONFIG } from "../config";
import { formatBytes } from "../boot/failures";
import {
  deviceMemoryGb,
  lowMemoryWarning,
  ROLE_COSTS,
} from "../roles/resources";
import { rolesAllowed } from "../roles/role";

/** The devnet faucet: a link only, the page never calls it. */
export const FAUCET_URL = "https://faucet.minaprotocol.com/";

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
      description={`${what} needs a devnet key.`}
      action={
        <Button asChild>
          <Link to="/key">
            Go to the key screen
            <ArrowRight aria-hidden />
          </Link>
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
        title="Resources"
        rows={[
          {
            label: "Prover circuits",
            value: <Measure value={formatBytes(cost.circuitBytes)} />,
          },
          // Each value a figure; what qualifies it is a quieter word.
          {
            label: "Peak tab memory",
            note: cost.peakBasis,
            value: (
              <Measure value={`~${(cost.peakBytes / GIB).toFixed(1)} GiB`} />
            ),
          },
          {
            label: "Prover ready after",
            value: <Measure value={`~${cost.setupSeconds} s`} />,
          },
          {
            label: "Device memory",
            value:
              reported === null ? (
                <span className="font-sans text-muted-foreground">
                  not reported (only Chromium browsers say)
                </span>
              ) : (
                // The browser reports powers of two, capped at 8: GiB, as
                // the peak above.
                <Measure value={reported >= 8 ? "≥8 GiB" : `${reported} GiB`} />
              ),
            note: reported === null ? undefined : "as the browser reports",
          },
        ]}
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

/** A form card beside a KeyValueList: the same ruled title bar, so the two
 * top edges match. For Card, CardHeader and CardContent. */
export const FORM_CARD = {
  card: "gap-0 py-0",
  header: "flex min-h-12 items-center border-b px-4 py-2.5",
  content: "flex flex-col gap-4 p-4",
} as const;

/** The one-role rule, said the same way on both role screens. */
export const ONE_ROLE =
  "A web node runs the SNARK worker or the block producer, not both. Choosing one stops the other.";

/** The docs section on the one-role rule. */
export const ONE_ROLE_DOCS_URL =
  "https://minanode.org/docs/web-node-connections/#one-role-per-tab";

/** The one-role rule with its docs link. */
export function OneRole() {
  return (
    <p
      className="flex items-start gap-2.5 text-sm text-muted-foreground"
      data-one-role
    >
      <Info aria-hidden className="mt-0.5 size-4 shrink-0 text-status-devnet" />
      <span>
        {ONE_ROLE}{" "}
        <ExternalLink href={ONE_ROLE_DOCS_URL}>Web node docs</ExternalLink>
      </span>
    </p>
  );
}
