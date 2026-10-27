import { inject, Injectable } from '@angular/core';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Store } from '@ngrx/store';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import {
  catchError,
  distinctUntilChanged,
  filter,
  map,
  Observable,
  of,
  startWith,
  throwError,
  timeout,
} from 'rxjs';
import { ChainBlock, LedgerAccount, QueryState } from './chain.types';

// Targets the native P7 schema. workIds are still empty, so job counts are
// unavailable; snarkJobs.length counts completed work bundles only.
export const BLOCK_FIELDS = `
  stateHash creator commandTransactionCount
  protocolState {
    previousStateHash
    consensusState { blockHeight slotSinceGenesis slot epoch }
    blockchainState { snarkedLedgerHash stagedLedgerHash date utcDate }
  }
  transactions {
    coinbase coinbaseReceiverAccount { publicKey }
    feeTransfer { recipient fee type }
    userCommands { hash from to amount fee memo nonce isDelegation failureReason }
    zkappCommands {
      hash failureReason { index failures }
      zkappCommand {
        feePayer { body { publicKey fee } }
        accountUpdates { body { publicKey callDepth } }
      }
    }
  }
  snarkJobs { prover fee }
`;

/** Not a fault to retry: the web node has no GraphQL. */
export const WEB_NODE_HAS_NO_GRAPHQL =
  'The web node in this tab has no GraphQL, so it cannot list blocks or ' +
  'look up accounts. Choose a native node in the node menu (top right).';

@Injectable({ providedIn: 'root' })
export class ChainService {
  private readonly http = inject(HttpClient);
  readonly node$ = inject(Store<MinaState>)
    .select(AppSelectors.activeNode)
    .pipe(
      filter((node): node is MinaNode => !!node),
      distinctUntilChanged(
        (a, b) => a.url === b.url && a.isWebNode === b.isWebNode,
      ),
    );

  bestChain(node: MinaNode): Observable<ChainBlock[]> {
    return this.query<{ bestChain: ChainBlock[] }>(
      node,
      `query RecentBlocks($maxLength: Int!) { bestChain(maxLength: $maxLength) { ${BLOCK_FIELDS} } }`,
      { maxLength: 20 },
    ).pipe(
      map(data =>
        [...data.bestChain].sort(
          (a, b) =>
            Number(b.protocolState.consensusState.blockHeight) -
            Number(a.protocolState.consensusState.blockHeight),
        ),
      ),
    );
  }

  block(node: MinaNode, stateHash: string): Observable<ChainBlock> {
    return this.query<{ block: ChainBlock }>(
      node,
      `query BlockDetails($stateHash: String!) { block(stateHash: $stateHash) { ${BLOCK_FIELDS} } }`,
      { stateHash },
    ).pipe(map(data => data.block));
  }

  account(node: MinaNode, publicKey: string): Observable<LedgerAccount | null> {
    return this.query<{ account: LedgerAccount | null }>(
      node,
      `
      query LedgerAccount($publicKey: PublicKey!) {
        account(publicKey: $publicKey) {
          publicKey nonce
          balance { total liquid locked blockHeight stateHash }
          delegateAccount { publicKey }
          timing { initialMinimumBalance cliffTime cliffAmount vestingPeriod vestingIncrement }
        }
      }`,
      { publicKey },
    ).pipe(map(data => data.account));
  }

  private query<T>(
    node: MinaNode,
    query: string,
    variables: object,
  ): Observable<T> {
    if (node.isWebNode) {
      return throwError(() => new Error(WEB_NODE_HAS_NO_GRAPHQL));
    }
    return this.http
      .post<{
        data?: T;
        errors?: { message: string }[];
      }>(`${node.url.replace(/\/$/, '')}/graphql`, { query, variables })
      .pipe(
        timeout(20000),
        map(response => {
          if (response.errors?.length) {
            throw new Error(
              response.errors.map(error => error.message).join('; '),
            );
          }
          if (!response.data)
            throw new Error('The node returned no GraphQL data.');
          return response.data;
        }),
      );
  }
}

export function queryState<T>(
  request: Observable<T>,
): Observable<QueryState<T>> {
  return request.pipe(
    map(data => ({ data, updatedAt: Date.now() })),
    catchError((error: unknown) => of({ error: queryError(error) })),
    startWith({ loading: true }),
  );
}

export function queryError(error: unknown): string {
  if (error instanceof HttpErrorResponse) {
    return error.status === 0
      ? 'Cannot reach this node. Check its URL and connection.'
      : `The node returned HTTP ${error.status}. Try again when it is available.`;
  }
  return error instanceof Error ? error.message : 'The node query failed.';
}
