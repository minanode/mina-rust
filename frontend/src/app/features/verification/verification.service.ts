import { inject, Injectable } from '@angular/core';
import { HttpClient, HttpParams } from '@angular/common/http';
import { Store } from '@ngrx/store';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { WebNodeService } from '@core/services/web-node.service';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { CONFIG } from '@shared/constants/config';
import {
  BehaviorSubject,
  debounceTime,
  distinctUntilChanged,
  EMPTY,
  filter,
  forkJoin,
  map,
  merge,
  scan,
  defer,
  Observable,
  of,
  shareReplay,
  startWith,
  switchMap,
  timer,
} from 'rxjs';
import {
  lookupResult,
  needsAnotherPage,
  pagesResult,
  PUBLIC_RECEIPT_PAGE_SIZE,
  PUBLIC_RECEIPTS_QUERY,
  PublicVerificationReceipt,
  publicLookupResult,
  RECEIPT_PAGE_SIZE,
  ReceiptLookupResult,
  VerificationReceipt,
  VerificationSnapshot,
} from './verification.types';

import { verificationPoll } from './verification-poll';

interface VerificationQuery {
  limit?: number;
  offset?: number;
  hashes?: string[];
}

/** One block's receipt, as a receipt component shows it. */
export interface ReceiptView {
  loading: boolean;
  error: boolean;
  browser: boolean;
  receipt?: VerificationReceipt;
  /** Every retained receipt was searched: no receipt means none retained. */
  complete: boolean;
  updatedAt?: number | null;
  unsupported?: boolean;
}

interface LookupState {
  receiptUpdatedAt: Map<string, number>;
  hashes: Set<string>;
  result: ReceiptLookupResult | null;
  error: boolean;
  browser: boolean;
  updatedAt: number | null;
  unsupported: boolean;
}

// Best-tip events drive refresh; this also catches checks finishing on a quiet tip.
const REFRESH_MS = 60000;

@Injectable({ providedIn: 'root' })
export class VerificationService {
  private readonly http = inject(HttpClient);
  private readonly web = inject(WebNodeService);
  private readonly store = inject(Store<MinaState>);
  private readonly node$ = this.store.select(AppSelectors.activeNode).pipe(
    filter(Boolean),
    distinctUntilChanged(
      (a, b) => a.url === b.url && a.isWebNode === b.isWebNode,
    ),
  );
  private readonly refresh$ = merge(
    timer(0, REFRESH_MS),
    this.store.select(AppSelectors.activeNodeDetails).pipe(
      map(details => details?.bestTipHash),
      distinctUntilChanged(),
      filter(Boolean),
    ),
  );

  /** Counters and newest receipts, retained across temporary RPC failures. */
  readonly state$ = this.node$.pipe(
    switchMap(node =>
      verificationPoll(
        this.refresh$,
        () => this.fetch(node, {}),
        20000,
        (data, previous) =>
          data.receipts.length > 0 || !previous?.receipts.length,
      ).pipe(map(state => ({ ...state, browser: !!node.isWebNode }))),
    ),
    shareReplay({ bufferSize: 1, refCount: true }),
  );

  // Batch visible hashes; a change in the batch must not cancel an in-flight RPC.
  private readonly wanted = new Map<string, number>();
  private readonly wanted$ = new BehaviorSubject<string[]>([]);
  private readonly lookup$: Observable<LookupState> = this.node$.pipe(
    switchMap(node =>
      verificationPoll(
        merge(
          this.refresh$,
          this.wanted$.pipe(
            debounceTime(20),
            distinctUntilChanged(
              (a, b) => a.length === b.length && a.every(h => b.includes(h)),
            ),
          ),
        ),
        () => {
          const hashes = this.wanted$.value;
          return hashes.length === 0
            ? EMPTY
            : this.lookup(node, hashes).pipe(
                map(result => ({ hashes: new Set(hashes), result })),
              );
        },
        30000,
      ).pipe(
        // Retain each visible block's successful receipt even when a later
        // valid answer no longer retains it. Drop hashes when they leave the page.
        scan(
          (previous: LookupState, state): LookupState => {
            const wanted = new Set(this.wanted$.value);
            const receiptUpdatedAt = new Map(
              [...previous.receiptUpdatedAt].filter(([hash]) =>
                wanted.has(hash),
              ),
            );
            const receipts = new Map(
              [...(previous.result?.receipts ?? [])].filter(([hash]) =>
                wanted.has(hash),
              ),
            );
            for (const [hash, receipt] of state.data?.result.receipts ?? []) {
              if (wanted.has(hash)) {
                receipts.set(hash, receipt);
                if (!state.error) receiptUpdatedAt.set(hash, state.updatedAt);
              }
            }
            const result = state.data
              ? { ...state.data.result, receipts }
              : null;
            return {
              receiptUpdatedAt,
              hashes: state.data?.hashes ?? wanted,
              result,
              error: state.error,
              unsupported: state.unsupported,
              updatedAt: state.updatedAt,
              browser: !!node.isWebNode,
            };
          },
          {
            receiptUpdatedAt: new Map<string, number>(),
            hashes: new Set<string>(),
            result: null,
            error: false,
            unsupported: false,
            updatedAt: null,
            browser: !!node.isWebNode,
          },
        ),
      ),
    ),
    shareReplay({ bufferSize: 1, refCount: true }),
  );

  /** The receipt of the block with this state hash, kept up to date. */
  receipt$(stateHash: string): Observable<ReceiptView> {
    return new Observable<ReceiptView>(subscriber => {
      this.want(stateHash, 1);
      const subscription = this.lookup$
        .pipe(
          map(state => view(state, stateHash)),
          startWith(LOADING),
        )
        .subscribe(subscriber);
      return () => {
        subscription.unsubscribe();
        this.want(stateHash, -1);
      };
    });
  }

  private want(hash: string, delta: number): void {
    const count = (this.wanted.get(hash) ?? 0) + delta;
    if (count > 0) this.wanted.set(hash, count);
    else this.wanted.delete(hash);
    this.wanted$.next([...this.wanted.keys()]);
  }

  private lookup(
    node: MinaNode,
    hashes: string[],
  ): Observable<ReceiptLookupResult> {
    const size = this.public(node)
      ? PUBLIC_RECEIPT_PAGE_SIZE
      : RECEIPT_PAGE_SIZE;
    const chunks: string[][] = [];
    for (let i = 0; i < hashes.length; i += size) {
      chunks.push(hashes.slice(i, i + size));
    }
    return forkJoin(chunks.map(chunk => this.lookupChunk(node, chunk))).pipe(
      map(results => ({
        receipts: new Map(results.flatMap(r => [...r.receipts])),
        complete: results.every(r => r.complete),
        method: results[0].method,
        requests: results.reduce((n, r) => n + r.requests, 0),
      })),
    );
  }

  private lookupChunk(
    node: MinaNode,
    hashes: string[],
  ): Observable<ReceiptLookupResult> {
    if (this.public(node)) return this.publicLookup(node, hashes);
    return this.fetch(node, {
      limit: RECEIPT_PAGE_SIZE,
      offset: 0,
      hashes,
    }).pipe(
      switchMap(snapshot =>
        snapshot.lookup
          ? of(lookupResult(snapshot))
          : // An older node ignores `hashes` and answers its newest page:
            // read on, a bounded number of pages.
            this.pages(node, hashes, [snapshot]),
      ),
    );
  }

  private pages(
    node: MinaNode,
    hashes: string[],
    pages: VerificationSnapshot[],
  ): Observable<ReceiptLookupResult> {
    if (!needsAnotherPage(hashes, pages)) {
      return of(pagesResult(hashes, pages));
    }
    return this.fetch(node, {
      limit: RECEIPT_PAGE_SIZE,
      offset: pages.length * RECEIPT_PAGE_SIZE,
    }).pipe(switchMap(page => this.pages(node, hashes, [...pages, page])));
  }

  /** A public node (`--public-graphql`) serves receipts through GraphQL. */
  private public(node: MinaNode): boolean {
    return !!CONFIG.publicApi && !node.isWebNode;
  }

  private publicLookup(
    node: MinaNode,
    stateHashes: string[],
  ): Observable<ReceiptLookupResult> {
    return this.http
      .post<{
        data?: { verificationReceipts: PublicVerificationReceipt[] } | null;
        errors?: { message: string }[];
      }>(`${(node.url ?? '').replace(/\/$/, '')}/graphql`, {
        query: PUBLIC_RECEIPTS_QUERY,
        variables: { stateHashes },
      })
      .pipe(
        map(response => {
          const receipts = response?.data?.verificationReceipts;
          if (response?.errors?.length || !Array.isArray(receipts))
            throw Object.assign(
              new Error(
                response?.errors?.map(e => e.message).join('; ') ||
                  'Verification receipts unavailable',
              ),
              {
                name: response?.errors?.some(e =>
                  /Cannot query field ["']verificationReceipts["']/.test(
                    e.message,
                  ),
                )
                  ? 'VerificationUnsupportedError'
                  : 'Error',
              },
            );
          return publicLookupResult(receipts);
        }),
      );
  }

  private fetch(
    node: MinaNode,
    query: VerificationQuery,
  ): Observable<VerificationSnapshot> {
    const request$: Observable<VerificationSnapshot> = node.isWebNode
      ? defer(() =>
          this.web.verificationQuery$(query.limit, query.offset, query.hashes),
        )
      : this.http.get<VerificationSnapshot>(
          `${(node.url ?? '').replace(/\/$/, '')}/stats/verification`,
          { params: httpParams(query) },
        );
    return request$.pipe(
      map(data => {
        if (!data || !Array.isArray(data.receipts))
          throw new Error('Verification receipts unavailable');
        return data;
      }),
    );
  }
}

const LOADING: ReceiptView = {
  loading: true,
  error: false,
  browser: false,
  complete: false,
};

function view(state: LookupState, hash: string): ReceiptView {
  if (!state.hashes.has(hash)) {
    // Asked for after this lookup started: the next one answers it.
    return { ...LOADING, browser: state.browser };
  }
  if (!state.result) {
    return {
      loading: !state.error && !state.unsupported,
      error: state.error,
      unsupported: state.unsupported,
      updatedAt: state.updatedAt,
      browser: state.browser,
      complete: false,
    };
  }
  return {
    loading: false,
    error: state.error,
    unsupported: state.unsupported,
    updatedAt: state.receiptUpdatedAt.get(hash) ?? state.updatedAt,
    browser: state.browser,
    receipt: state.result.receipts.get(hash),
    complete: state.result.complete,
  };
}

function httpParams(query: VerificationQuery): HttpParams {
  let params = new HttpParams();
  if (query.limit !== undefined) params = params.set('limit', query.limit);
  if (query.offset !== undefined) params = params.set('offset', query.offset);
  if (query.hashes?.length)
    params = params.set('hashes', query.hashes.join(','));
  return params;
}
