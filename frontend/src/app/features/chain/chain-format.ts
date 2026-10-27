import type { ChainBlock, UserCommand, ZkappCommand } from './chain.types';

/** GraphQL amounts are integer nanomina. Do not round through Number. */
export function formatMina(value: string | null | undefined): string {
  if (value == null || !/^\d+$/.test(value)) return 'Unavailable';
  const amount = BigInt(value);
  const whole = (amount / 1_000_000_000n).toLocaleString('en-US');
  const fraction = (amount % 1_000_000_000n)
    .toString()
    .padStart(9, '0')
    .replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole;
}

export function commandFees(block: ChainBlock): string {
  const fees = [
    ...block.transactions.userCommands.map(c => c.fee),
    ...block.transactions.zkappCommands.map(
      c => c.zkappCommand.feePayer.body.fee,
    ),
  ];
  if (fees.some(fee => !/^\d+$/.test(fee))) return 'Unavailable';
  return formatMina(
    fees.reduce((sum, fee) => sum + BigInt(fee), 0n).toString(),
  );
}

export function blockCounts(block: ChainBlock) {
  const delegations = block.transactions.userCommands.filter(
    c => c.isDelegation,
  ).length;
  return {
    payments: block.transactions.userCommands.length - delegations,
    delegations,
    zkapps: block.transactions.zkappCommands.length,
    works: block.snarkJobs.length,
  };
}

/** A command found in a block by its hash, for the `tx` permalink (R28). */
export type FoundCommand =
  | { kind: 'Payment' | 'Delegation'; command: UserCommand }
  | { kind: 'zkApp command'; command: ZkappCommand };

export function findCommand(
  block: ChainBlock,
  hash: string | null | undefined,
): FoundCommand | null {
  if (!hash) return null;
  const user = block.transactions.userCommands.find(c => c.hash === hash);
  if (user) {
    return {
      kind: user.isDelegation ? 'Delegation' : 'Payment',
      command: user,
    };
  }
  const zkapp = block.transactions.zkappCommands.find(c => c.hash === hash);
  return zkapp ? { kind: 'zkApp command', command: zkapp } : null;
}

/**
 * Slots per epoch on Mina mainnet and devnet, from their genesis protocol
 * constants. The soak node's GraphQL answered
 * daemonStatus.consensusConfiguration.slotsPerEpoch = 7140 on 2026-10-21.
 */
export const SLOTS_PER_EPOCH = 7140;

/**
 * The global slot since the hard fork: the one definition of a block's
 * slot on every page (R28, after R27 found 1,004,393 here and 45,953 on
 * Nodes > Bootstrap for one block). The node's sync statistics, its peers'
 * tips and the block production pages all count from the hard fork. GraphQL
 * gives it as the epoch and the slot within it; its `slotSinceGenesis`
 * counts across all forks and is shown only under that name.
 */
export function slotSinceHardFork(consensus: {
  epoch: string;
  slot: string;
}): number | null {
  if (!/^\d+$/.test(consensus.epoch) || !/^\d+$/.test(consensus.slot)) {
    return null;
  }
  return Number(consensus.epoch) * SLOTS_PER_EPOCH + Number(consensus.slot);
}
