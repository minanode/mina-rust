import { Injectable } from '@angular/core';
import { Actions, createEffect, ofType } from '@ngrx/effects';
import { Store } from '@ngrx/store';
import { Effect } from '@mina-rust/shared';
import { catchError, EMPTY, map, switchMap } from 'rxjs';
import { AppActions } from '@app/app.actions';
import { MinaRustBaseEffect } from '@shared/base-classes/mina-rust-base.effect';
import {
  DASHBOARD_SPLITS_CLOSE,
  DASHBOARD_SPLITS_GET_SPLITS,
  DASHBOARD_SPLITS_GET_SPLITS_SUCCESS,
  DashboardSplitsActions,
  DashboardSplitsGetSplitsSuccess,
} from '@network/splits/dashboard-splits.actions';
import { DashboardSplitsService } from '@network/splits/dashboard-splits.service';
import { MinaState, selectMinaState } from '@app/app.setup';
import { MinaErrorType } from '@shared/types/error-preview/mina-error-type.enum';
import { addError } from '@shared/constants/store-functions';

@Injectable({
  providedIn: 'root',
})
export class DashboardSplitsEffects extends MinaRustBaseEffect<
  DashboardSplitsActions | ReturnType<typeof AppActions.changeActiveNode>
> {
  readonly getSplits$: Effect;
  private active = false;

  constructor(
    private actions$: Actions,
    private splitService: DashboardSplitsService,
    store: Store<MinaState>,
  ) {
    super(store, selectMinaState);

    this.getSplits$ = createEffect(() =>
      this.actions$.pipe(
        ofType(
          DASHBOARD_SPLITS_GET_SPLITS,
          DASHBOARD_SPLITS_CLOSE,
          AppActions.changeActiveNode,
        ),
        this.latestActionState<
          | DashboardSplitsActions
          | ReturnType<typeof AppActions.changeActiveNode>
        >(),
        switchMap(({ action, state }) => {
          if (action.type === DASHBOARD_SPLITS_CLOSE) {
            this.active = false;
            return EMPTY;
          }
          if (action.type === DASHBOARD_SPLITS_GET_SPLITS) {
            this.active = true;
          }
          if (!this.active) {
            return EMPTY;
          }
          const node = state.app.activeNode;
          return this.splitService.getPeers(node).pipe(
            map(payload => ({
              type: DASHBOARD_SPLITS_GET_SPLITS_SUCCESS,
              payload,
            })),
            catchError(error => [
              addError(error, MinaErrorType.RUST),
              new DashboardSplitsGetSplitsSuccess({
                peers: [],
                links: [],
                node: node.name,
              }),
            ]),
          );
        }),
      ),
    );
  }
}
