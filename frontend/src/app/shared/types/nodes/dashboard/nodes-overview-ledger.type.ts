export interface NodesOverviewLedger {
  stakingEpoch: NodesOverviewLedgerEpochStep;
  nextEpoch: NodesOverviewLedgerEpochStep;
  rootSnarked: NodesOverviewLedgerEpochStep;
  rootStaged: NodesOverviewRootStagedLedgerStep;
}

export interface NodesOverviewLedgerEpochStep {
  state: NodesOverviewLedgerStepState | any;
  snarked: NodesOverviewSnarkedLedgerStep;
  totalTime: number;
  /** Loaded from saved state at a restart, not fetched (F-dash3). */
  fromStore?: boolean;
}

export interface NodesOverviewRootStagedLedgerStep {
  state: NodesOverviewLedgerStepState;
  staged: NodesOverviewStagedLedgerStep;
  totalTime: number;
  /** Loaded from saved state at a restart, not fetched (F-dash3). */
  fromStore?: boolean;
}

export enum NodesOverviewLedgerStepState {
  PENDING = 'pending',
  LOADING = 'loading',
  SUCCESS = 'success',
}

export interface NodesOverviewSnarkedLedgerStep {
  fetchHashesStart: number;
  fetchHashesEnd: number;
  fetchHashesDuration: number;
  fetchHashesPassedTime: number;
  fetchAccountsStart: number;
  fetchAccountsEnd: number;
  fetchAccountsDuration: number;
  fetchAccountsPassedTime: number;
}

export interface NodesOverviewStagedLedgerStep {
  fetchPartsStart: number;
  fetchPartsEnd: number;
  fetchPartsDuration: number;
  fetchPassedTime: number;
  reconstructStart: number;
  reconstructEnd: number;
  reconstructDuration: number;
  reconstructPassedTime: number;
}
