/**
 * The block producer: before it runs, the key's stake in this epoch's
 * staking ledger and the next, when stake delegated today would count
 * (from the node's own slot time and epoch length), what it costs the
 * device, and the passphrase that carries the key across the restart;
 * while it runs, the dashboard's producer view: won slots, each attempt's
 * stages with its proof, and each block's fate.
 */

import * as React from "react";
import { Box, Play, Power, TriangleAlert } from "lucide-react";

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
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Amount } from "@minanode/design-system/patterns/amount";
import { Hash } from "@minanode/design-system/patterns/hash";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { Signal } from "@minanode/design-system/patterns/signal";
import {
  formatCount,
  formatHeight,
  formatMina,
  utcDateTime,
} from "@minanode/node-client";
import { usePoll, useProducerStats } from "@minanode/node-client/react";
import { Loaded } from "minanode-dashboard/components/states";
import { Producing } from "minanode-dashboard/pages/producer";

import { useBoot } from "../app/boot-context";
import {
  days,
  epochClock,
  stakeVerdict,
  type StakeVerdict,
} from "../roles/epoch";
import { useKeys, useRoleNode } from "../roles/hooks";
import { MIN_PASSPHRASE, passphraseProblem } from "../roles/keys";
import { restartWithKey } from "../roles/restart";
import { VIEWER } from "../roles/role";
import { RestartConfirm } from "./restart-confirm";
import {
  DevnetOnly,
  NeedKey,
  ONE_ROLE,
  RoleCost,
  RolesGate,
} from "./roles-common";

const STAKE_POLL_MS = 60_000;
const STATS_POLL_MS = 10_000;

export function BlockProducerPage() {
  const boot = useBoot();
  const { key } = useKeys();
  const running = boot.role.kind === "producer";
  return (
    <>
      <PageHeader
        eyebrow="devnet role"
        title="Block producer"
        description="Produces blocks in this tab for the slots its key wins, and proves each one here. A key wins slots only with stake."
        aside={
          running ? (
            <Signal tone="ok" pulse className="text-sm">
              On in this tab
            </Signal>
          ) : null
        }
      />
      <RolesGate>
        <div className="flex flex-col gap-6">
          <DevnetOnly />
          {running ? (
            <Running />
          ) : key ? (
            <Setup />
          ) : (
            <NeedKey what="Producing blocks" />
          )}
        </div>
      </RolesGate>
    </>
  );
}

/** The key's stake and the epoch clock, read from the node. */
function useStakeView(publicKey: string | null) {
  const node = useRoleNode();
  const stake = usePoll(
    node?.available && publicKey ? `stake:${publicKey}` : null,
    () => node!.stake(publicKey!),
    STAKE_POLL_MS,
  );
  const clock = usePoll(
    node?.available ? "epoch-clock" : null,
    async () => {
      const [consensus, time] = await Promise.all([
        node!.consensus(),
        node!.consensusTime(),
      ]);
      return consensus && time
        ? { consensus, clock: epochClock(consensus, time) }
        : null;
    },
    STAKE_POLL_MS,
  );
  const verdict = stakeVerdict(
    stake.data ?? null,
    clock.data?.clock ?? null,
    clock.data?.consensus ?? null,
  );
  return { stake, clock, verdict };
}

function VerdictAlert({ verdict }: { verdict: StakeVerdict }) {
  switch (verdict.kind) {
    case "none":
      return (
        <Alert variant="destructive" data-verdict="none">
          <TriangleAlert aria-hidden />
          <AlertTitle>This key will not produce blocks</AlertTitle>
          <AlertDescription>
            <p>
              It has no stake in this epoch's staking ledger or the next. A key
              with no stake never wins a slot, however long it runs. You can
              still start it, to see the producer at work.
            </p>
          </AlertDescription>
        </Alert>
      );
    case "next":
      return (
        <Alert variant="caution" data-verdict="next">
          <TriangleAlert aria-hidden />
          <AlertTitle>Not this epoch</AlertTitle>
          <AlertDescription>
            <p>
              Its stake is in the next epoch ledger only: it can win slots from
              epoch {formatCount(verdict.epoch)}, in about{" "}
              {days(verdict.fromMs - Date.now())} ({utcDateTime(verdict.fromMs)}
              ). Until then it will not produce.
            </p>
          </AlertDescription>
        </Alert>
      );
    case "now":
      return (
        <Alert variant="brand" data-verdict="now">
          <Box aria-hidden />
          <AlertTitle>This key has stake this epoch</AlertTitle>
          <AlertDescription>
            <p>
              At its share of the stake it can expect about{" "}
              {verdict.expectedPerEpoch < 10
                ? verdict.expectedPerEpoch.toFixed(1)
                : formatCount(Math.round(verdict.expectedPerEpoch))}{" "}
              won slots an epoch. Each needs the tab open and a block proof in
              time.
            </p>
          </AlertDescription>
        </Alert>
      );
    case "unknown":
      return (
        <p className="text-sm text-muted-foreground" data-verdict="unknown">
          Reading the key's stake from the epoch ledgers…
        </p>
      );
  }
}

function StakeFacts({ publicKey }: { publicKey: string }) {
  const { stake, clock, verdict } = useStakeView(publicKey);
  const s = stake.data;
  const c = clock.data;
  const amount = (n: bigint | null | undefined) =>
    n === null || n === undefined ? (
      "Not read yet"
    ) : (
      <Amount unit="MINA" value={formatMina(n)} />
    );
  return (
    <div className="flex flex-col gap-4">
      <VerdictAlert verdict={verdict} />
      <KeyValueList
        title="Stake"
        rows={[
          {
            label: `Epoch ${s ? formatCount(s.epoch) : "–"}, staking ledger`,
            value: amount(s?.staking.delegatedStake),
            tone: "brand",
          },
          {
            label: "Next epoch ledger",
            value: amount(s?.next.delegatedStake),
          },
          {
            label: "Delegators now",
            value:
              s?.staking.delegators != null
                ? formatCount(s.staking.delegators)
                : "–",
          },
          {
            label: "At the best tip",
            value: s ? formatHeight(s.bestTipHeight) : "–",
          },
        ]}
        footer="Its own balance and its delegators', in each epoch ledger of the node's best tip."
      />
      <KeyValueList
        title="When delegated stake counts"
        data-delegation-days={
          c
            ? (
                (c.clock.delegationCountsFromMs - Date.now()) /
                86_400_000
              ).toFixed(2)
            : undefined
        }
        rows={[
          {
            label: "Slot",
            value: c ? `${Math.round(c.consensus.slotMs / 1000)} s` : "–",
          },
          {
            label: "Slots an epoch",
            value: c ? formatCount(c.consensus.slotsPerEpoch) : "–",
          },
          {
            label: "Epoch length",
            value: c ? days(c.clock.epochMs) : "–",
          },
          {
            label: `Epoch ${c ? formatCount(c.clock.epoch) : "–"} ends`,
            value: c ? `in ${days(c.clock.epochEndMs - Date.now())}` : "–",
          },
          {
            label: "Stake delegated today counts",
            value: c
              ? `from epoch ${formatCount(c.clock.delegationEpoch)} at the earliest, in about ${days(c.clock.delegationCountsFromMs - Date.now())}`
              : "–",
            tone: "brand",
          },
        ]}
        footer={
          c
            ? `A delegation counts only after the epoch transitions twice: stake delegated in epoch ${formatCount(c.clock.epoch)} enters the next epoch ledger at the change to epoch ${formatCount(c.clock.epoch + 1)}, which becomes the staking ledger at the change to epoch ${formatCount(c.clock.delegationEpoch)} (${utcDateTime(c.clock.delegationCountsFromMs)}). That is the earliest: the node takes the snarked ledger at the change, so a delegation in one of the epoch's last blocks can wait an epoch more.`
            : "From the node's consensus constants and its current slot."
        }
      />
    </div>
  );
}

function Setup() {
  const boot = useBoot();
  const { key, saved } = useKeys();
  const [passphrase, setPassphrase] = React.useState("");
  const [confirming, setConfirming] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  if (!key) return null;
  const problem = passphraseProblem(passphrase);
  const switching = boot.role.kind === "worker";

  async function start() {
    if (!key) return;
    setBusy(true);
    try {
      // The key crosses the restart encrypted, in this tab only; the boot
      // deletes it once it is unlocked. Refused if storage won't keep it.
      await restartWithKey(
        { kind: "producer", publicKey: key.publicKey },
        key,
        saved,
        { kind: "carry", passphrase },
      );
      setPassphrase("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }

  return (
    <div className="grid items-start gap-6 lg:grid-cols-2">
      <div className="flex flex-col gap-6">
        <Card>
          <CardHeader>
            <CardTitle>Start the block producer</CardTitle>
            <CardDescription>
              The node starts again with this key as its producer. The key
              crosses the restart encrypted with a passphrase you type now, in
              this tab only, and you type it again once the node restarts.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-foreground">
                Producer key
              </span>
              <Hash value={key.publicKey} label="producer key" />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="producer-passphrase">
                Passphrase for the restart
              </Label>
              <Input
                id="producer-passphrase"
                type="password"
                autoComplete="new-password"
                value={passphrase}
                onChange={e => setPassphrase(e.target.value)}
              />
              <p className="text-sm text-muted-foreground">
                At least {MIN_PASSPHRASE} characters.
              </p>
            </div>
            <p className="text-sm text-muted-foreground">{ONE_ROLE}</p>
            {error ? <p className="text-sm text-destructive">{error}</p> : null}
            {confirming ? (
              <InlineConfirm
                label="Confirm starting the block producer"
                message={
                  switching
                    ? "This stops the SNARK worker and starts the node again as the block producer for this key."
                    : "The node starts again as the block producer for this key. Keep the tab open: a closed tab misses its slots."
                }
                confirmLabel={
                  switching ? "Stop the worker and produce" : "Start producing"
                }
                busy={busy}
                busyLabel="Encrypting…"
                onConfirm={() => void start()}
                onCancel={() => setConfirming(false)}
              />
            ) : (
              <Button
                type="button"
                className="w-fit"
                disabled={problem !== null}
                data-action="start-producer"
                onClick={() => setConfirming(true)}
              >
                <Play aria-hidden />
                Start the block producer
              </Button>
            )}
          </CardContent>
        </Card>
        <RoleCost kind="producer" />
      </div>
      <StakeFacts publicKey={key.publicKey} />
    </div>
  );
}

function Running() {
  const boot = useBoot();
  const stats = useProducerStats(STATS_POLL_MS);
  const [confirming, setConfirming] = React.useState(false);
  const publicKey = boot.role.kind === "producer" ? boot.role.publicKey : null;
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        {confirming ? (
          <RestartConfirm
            label="Confirm stopping the block producer"
            message="The node starts again without the producer. A won slot ahead is then missed."
            confirmLabel="Stop producing"
            role={VIEWER}
            onCancel={() => setConfirming(false)}
          />
        ) : (
          <Button
            type="button"
            variant="outline"
            data-action="stop-producer"
            onClick={() => setConfirming(true)}
          >
            <Power aria-hidden />
            Stop the block producer
          </Button>
        )}
      </div>
      {publicKey ? <StakeFacts publicKey={publicKey} /> : null}
      <Loaded polled={stats} what="block production" rows={4}>
        {data =>
          data === null ? (
            <p className="text-sm text-muted-foreground">
              The producer reports once the node has a best tip.
            </p>
          ) : (
            <Producing stats={data} />
          )
        }
      </Loaded>
    </>
  );
}
