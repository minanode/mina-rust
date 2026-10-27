import { inject, Injectable } from '@angular/core';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Store } from '@ngrx/store';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { distinctUntilChanged, filter, Observable, timeout } from 'rxjs';
import {
  IncludedWork,
  WorkerChange,
  WorkerControl,
} from './snarks-worker.types';

/** The native node's SNARK worker control and included-work log (P5.4). */
@Injectable({ providedIn: 'root' })
export class SnarksWorkerService {
  private readonly http = inject(HttpClient);
  readonly node$ = inject(Store<MinaState>)
    .select(AppSelectors.activeNode)
    .pipe(
      filter((node): node is MinaNode => !!node),
      distinctUntilChanged(
        (a, b) => a.url === b.url && a.isWebNode === b.isWebNode,
      ),
    );

  control(node: MinaNode): Observable<WorkerControl> {
    return this.http
      .get<WorkerControl>(node.url + '/snarker/control')
      .pipe(timeout(15000));
  }

  /**
   * The node takes a change only with this header and JSON, from a page of
   * this machine or an origin it lists (`--worker-control-origin`), so that
   * another site open in the browser can't redirect the fees.
   */
  change(node: MinaNode, change: WorkerChange): Observable<WorkerControl> {
    return this.http
      .post<WorkerControl>(node.url + '/snarker/control', change, {
        headers: {
          'Content-Type': 'application/json',
          'X-Mina-Worker-Control': '1',
        },
      })
      .pipe(timeout(30000));
  }

  included(node: MinaNode): Observable<IncludedWork> {
    return this.http
      .get<IncludedWork>(node.url + '/snarker/included')
      .pipe(timeout(15000));
  }
}

/** The node's reason, or what went wrong reaching it. */
export function workerError(error: unknown): string {
  if (error instanceof HttpErrorResponse) {
    const reason = error.error?.error ?? error.error;
    if (typeof reason === 'string' && reason) {
      return reason;
    }
    if (error.status === 404) {
      return 'This node has no worker control (an older build, or a public API).';
    }
    if (error.status === 0) {
      return 'The node did not answer.';
    }
    return `The node answered ${error.status} ${error.statusText}.`;
  }
  if (error instanceof Error && error.name === 'TimeoutError') {
    return 'The node did not answer in time.';
  }
  return String(error);
}

/** Nanomina as MINA, without trailing zeros. */
export function formatMina(nanomina: number | null | undefined): string {
  if (nanomina === null || nanomina === undefined) {
    return '–';
  }
  const whole = Math.floor(nanomina / 1e9);
  const fraction = Math.round(nanomina - whole * 1e9);
  if (!fraction) {
    return String(whole);
  }
  return `${whole}.${String(fraction).padStart(9, '0').replace(/0+$/, '')}`;
}

/** The explorer's page for a block height on the node's network. */
export function explorerBlockUrl(network: string, height: number): string {
  const name = network === 'mainnet' ? 'mainnet' : 'devnet';
  return `https://o1-labs.github.io/mina-explorer/#/block/${height}?network=${name}`;
}
