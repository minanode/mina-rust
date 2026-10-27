export interface UserCommand {
  hash: string;
  from: string;
  to: string;
  amount: string | null;
  fee: string;
  memo: string;
  nonce: number;
  isDelegation: boolean;
  failureReason: string | null;
}

export interface ZkappCommand {
  hash: string;
  failureReason: { index: string; failures: string[] }[] | null;
  zkappCommand: {
    feePayer: { body: { publicKey: string; fee: string } };
    accountUpdates: { body: { publicKey: string; callDepth: number } }[];
  };
}

export interface ChainBlock {
  stateHash: string;
  creator: string;
  commandTransactionCount: number;
  protocolState: {
    previousStateHash: string;
    consensusState: {
      blockHeight: string;
      slotSinceGenesis: string;
      slot: string;
      epoch: string;
    };
    blockchainState: {
      snarkedLedgerHash: string;
      stagedLedgerHash: string;
      date: string;
      utcDate: string;
    };
  };
  transactions: {
    coinbase: string;
    coinbaseReceiverAccount: { publicKey: string } | null;
    feeTransfer: { recipient: string; fee: string; type: string }[];
    userCommands: UserCommand[];
    zkappCommands: ZkappCommand[];
  };
  snarkJobs: { prover: string; fee: string }[];
}

export interface LedgerAccount {
  publicKey: string;
  nonce: string;
  balance: {
    total: string;
    liquid: string | null;
    locked: string | null;
    blockHeight: string;
    stateHash: string | null;
  };
  delegateAccount: { publicKey: string } | null;
  timing: {
    initialMinimumBalance: string | null;
    cliffTime: string | null;
    cliffAmount: string | null;
    vestingPeriod: string | null;
    vestingIncrement: string | null;
  };
}

export interface QueryState<T> {
  data?: T;
  loading?: boolean;
  error?: string;
  updatedAt?: number;
}
