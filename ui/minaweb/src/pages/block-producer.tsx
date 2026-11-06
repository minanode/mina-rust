/**
 * The block producer: before it runs, the key's stake in this epoch's
 * staking ledger and the next, when stake delegated today would count
 * (from the node's own slot time and epoch length), what it costs the
 * device, the passphrase of a key saved here but not held, and the key
 * its coinbase goes to if not its own;
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
  CardHeader,
  CardTitle,
} from "@minanode/design-system/components/card";
import { Input } from "@minanode/design-system/components/input";
import { Label } from "@minanode/design-system/components/label";
import { Amount, Measure } from "@minanode/design-system/patterns/amount";
import { PageSection } from "@minanode/design-system/patterns/page-section";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { Hash } from "@minanode/design-system/patterns/hash";
import { InlineConfirm } from "@minanode/design-system/patterns/inline-confirm";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";
import { cn } from "@minanode/design-system/lib/utils";
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
import { HandoffError, handoffSupported } from "../roles/handoff";
import { useKeyGlue, useKeys, useRoleNode } from "../roles/hooks";
import {
  keyStore,
  looksLikePublicKey,
  MIN_PASSPHRASE,
  passphraseProblem,
  type KeyGlue,
} from "../roles/keys";
import { restartWithKey } from "../roles/restart";
import { VIEWER } from "../roles/role";
import { RestartConfirm } from "./restart-confirm";
import {
  FORM_CARD,
  NeedKey,
  OneRole,
  RoleCost,
  RolesGate,
} from "./roles-common";

/** The block producer's docs page. */
export const PRODUCER_DOCS_URL = "https://minanode.org/docs/block-producer/";

const STAKE_POLL_MS = 60_000;
const STATS_POLL_MS = 10_000;

/** A typed coinbase receiver: none when empty, else a public key that
 * `run` takes (the node's own check, with its checksum). */
export function coinbaseReceiver(
  text: string,
  glue: Pick<KeyGlue, "account_public_key_valid"> | null,
): { receiver: string | null; problem: string | null } {
  const receiver = text.trim();
  if (!receiver) return { receiver: null, problem: null };
  return looksLikePublicKey(receiver) &&
    glue?.account_public_key_valid(receiver) === true
    ? { receiver, problem: null }
    : { receiver: null, problem: "Not a public key (B62…)." };
}

export function BlockProducerPage() {
  const boot = useBoot();
  const { key, saved } = useKeys();
  const running = boot.role.kind === "producer";
  // With no key the page is one small panel: it keeps to the width of the
  // key and send forms; set up or running, it takes the full width. The
  // header runs to the content edge either way.
  const keyless = !running && !key && !saved;
  return (
    <div className="flex flex-col gap-6 lg:gap-8">
      <PageHeader
        eyebrow="Devnet"
        title="Block producer"
        aside={
          running ? (
            <Signal tone="ok" pulse>
              On in this tab
            </Signal>
          ) : null
        }
      />
      <RolesGate>
        <div className={cn("flex flex-col gap-6", keyless && "max-w-3xl")}>
          {running ? (
            <Running />
          ) : key || saved ? (
            <Setup />
          ) : (
            <NeedKey what="Producing blocks" />
          )}
        </div>
      </RolesGate>
    </div>
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
        // A new key's usual state, not a failure: caution, not red.
        <Alert variant="caution" data-verdict="none">
          <TriangleAlert aria-hidden />
          <AlertTitle>This key will not produce blocks</AlertTitle>
          <AlertDescription>
            <p>No stake this epoch or next.</p>
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
              ).
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
              won slots an epoch.
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

/** The key's stake and when delegated stake counts: stacked beside the
 * setup form, side by side under the running producer. */
function StakeFacts({
  view: { stake, clock },
  layout,
}: {
  view: ReturnType<typeof useStakeView>;
  layout: "stack" | "pair";
}) {
  const s = stake.data;
  const c = clock.data;
  const amount = (n: bigint | null | undefined) =>
    n === null || n === undefined ? (
      <span className="font-sans">Not read yet</span>
    ) : (
      <Amount unit="MINA" value={formatMina(n)} />
    );
  return (
    <div
      className={
        layout === "stack"
          ? "flex flex-col gap-4"
          : "grid items-start gap-6 lg:grid-cols-2"
      }
    >
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
              s?.staking.delegators != null ? (
                <span
                  className={
                    s.staking.delegators === 0
                      ? "text-muted-foreground"
                      : undefined
                  }
                >
                  {formatCount(s.staking.delegators)}
                </span>
              ) : (
                "–"
              ),
          },
          {
            label: "At the best tip",
            value: s ? formatHeight(s.bestTipHeight) : "–",
          },
        ]}
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
            label: "Slot length",
            value: (
              <Measure
                value={c ? `${Math.round(c.consensus.slotMs / 1000)} s` : null}
              />
            ),
          },
          {
            label: "Slots per epoch",
            value: c ? formatCount(c.consensus.slotsPerEpoch) : "–",
          },
          {
            label: "Epoch length",
            value: <Measure value={c ? days(c.clock.epochMs) : null} />,
          },
          {
            label: `Epoch ${c ? formatCount(c.clock.epoch) : "–"} ends in`,
            value: (
              <Measure
                value={c ? days(c.clock.epochEndMs - Date.now()) : null}
              />
            ),
          },
          {
            label: "Earliest epoch for stake delegated now",
            value: c ? formatCount(c.clock.delegationEpoch) : "–",
            tone: "brand",
          },
          {
            label: `Epoch ${c ? formatCount(c.clock.delegationEpoch) : "–"} starts in`,
            value: (
              <Measure
                value={
                  c
                    ? `~${days(c.clock.delegationCountsFromMs - Date.now())}`
                    : null
                }
              />
            ),
          },
        ]}
        footer={
          <ExternalLink href={PRODUCER_DOCS_URL}>
            Block producer docs
          </ExternalLink>
        }
      />
    </div>
  );
}

function Setup() {
  const boot = useBoot();
  const glue = useKeyGlue();
  const { key, saved } = useKeys();
  const [passphrase, setPassphrase] = React.useState("");
  const [typedReceiver, setTypedReceiver] = React.useState("");
  const [confirming, setConfirming] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  // Set once the key can't cross the restart by itself: it then crosses
  // encrypted with a passphrase typed here.
  const [fallback, setFallback] = React.useState(() => !handoffSupported());
  const publicKey = key?.publicKey ?? saved?.publicKey;
  const view = useStakeView(publicKey ?? null);
  if (!publicKey) return null;
  // Saved in this browser, not held: its passphrase unlocks it first.
  const locked = !key;
  const receiver = coinbaseReceiver(typedReceiver, glue);
  const problem =
    receiver.problem ??
    (locked
      ? passphrase
        ? null
        : "Type the saved key's passphrase."
      : fallback
        ? passphraseProblem(passphrase)
        : null);
  const switching = boot.role.kind === "worker";

  async function start() {
    // The field can change while the confirmation is open: its hint then
    // says what is wrong.
    if (receiver.problem) {
      setConfirming(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      let held = key;
      if (!held) {
        await keyStore.unlockSaved(passphrase, glue ?? undefined);
        setPassphrase("");
        held = keyStore.getSnapshot().key;
        if (!held) throw new Error("The saved key didn't unlock.");
        // The restart then needs a passphrase of its own.
        if (fallback) {
          setBusy(false);
          setConfirming(false);
          return;
        }
      }
      await restartWithKey(
        {
          kind: "producer",
          publicKey: held.publicKey,
          ...(receiver.receiver ? { coinbaseReceiver: receiver.receiver } : {}),
        },
        held,
        fallback ? { kind: "carry", passphrase } : null,
      );
      setPassphrase("");
    } catch (e) {
      if (e instanceof HandoffError) {
        setFallback(true);
        setConfirming(false);
      }
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }

  return (
    <div className="grid items-start gap-6 lg:grid-cols-2">
      {/* On a phone the verdict comes before the button it qualifies. */}
      <div className="lg:hidden">
        <VerdictAlert verdict={view.verdict} />
      </div>
      <div className="flex flex-col gap-6">
        <Card className={FORM_CARD.card}>
          <CardHeader className={FORM_CARD.header}>
            <CardTitle>Settings</CardTitle>
          </CardHeader>
          <CardContent className={FORM_CARD.content}>
            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-foreground">
                Producer key
              </span>
              <Hash value={publicKey} label="producer key" />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="producer-coinbase-receiver">
                Coinbase receiver{" "}
                <span className="font-normal text-muted-foreground">
                  optional
                </span>
              </Label>
              <Input
                id="producer-coinbase-receiver"
                value={typedReceiver}
                onChange={e => setTypedReceiver(e.target.value)}
                placeholder="B62…"
                className="font-mono"
                spellCheck={false}
                autoComplete="off"
                aria-invalid={receiver.problem ? true : undefined}
                aria-describedby="producer-coinbase-receiver-hint"
              />
              <p
                id="producer-coinbase-receiver-hint"
                className={
                  receiver.problem
                    ? "text-sm text-destructive"
                    : "text-sm text-muted-foreground"
                }
              >
                {receiver.problem ?? "Empty: the producer key receives it."}
              </p>
            </div>
            {locked ? (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="producer-saved-passphrase">
                  Saved key's passphrase
                </Label>
                <Input
                  id="producer-saved-passphrase"
                  type="password"
                  autoComplete="current-password"
                  value={passphrase}
                  onChange={e => setPassphrase(e.target.value)}
                  aria-invalid={error ? true : undefined}
                />
              </div>
            ) : fallback ? (
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
                {/* The confirm says the node restarts with the key. */}
                <p className="text-sm text-muted-foreground">
                  At least {MIN_PASSPHRASE} characters. Asked again after the
                  restart.
                </p>
              </div>
            ) : null}
            {/* The confirm says what stops: the note goes while it asks. */}
            {confirming ? null : <OneRole />}
            {error ? <p className="text-sm text-destructive">{error}</p> : null}
            {confirming ? (
              <InlineConfirm
                label="Confirm starting the block producer"
                message={
                  switching
                    ? "This stops the SNARK worker and starts the node again as the block producer for this key."
                    : "The node restarts as the block producer for this key. Keep this tab open: a closed tab misses its slots."
                }
                confirmLabel={
                  switching
                    ? "Stop the worker and produce"
                    : "Restart as block producer"
                }
                tone={switching ? "destructive" : "caution"}
                busy={busy}
                busyLabel={locked ? "Unlocking…" : "Restarting…"}
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
      <div className="flex flex-col gap-4">
        <div className="hidden lg:block">
          <VerdictAlert verdict={view.verdict} />
        </div>
        <StakeFacts view={view} layout="stack" />
      </div>
    </div>
  );
}

function Running() {
  const boot = useBoot();
  const stats = useProducerStats(STATS_POLL_MS);
  const [confirming, setConfirming] = React.useState(false);
  const publicKey = boot.role.kind === "producer" ? boot.role.publicKey : null;
  const view = useStakeView(publicKey);
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        {confirming ? (
          <RestartConfirm
            label="Confirm stopping the block producer"
            message="The node starts again without the producer. A won slot ahead is then missed."
            confirmLabel="Stop producing"
            role={VIEWER}
            tone="destructive"
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
      {/* Live state first; the stake and its clock under it. */}
      {publicKey ? <VerdictAlert verdict={view.verdict} /> : null}
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
      {/* Its own section, so it doesn't read as part of Won slots. */}
      {publicKey ? (
        <PageSection title="Delegation">
          <StakeFacts view={view} layout="pair" />
        </PageSection>
      ) : null}
    </>
  );
}
