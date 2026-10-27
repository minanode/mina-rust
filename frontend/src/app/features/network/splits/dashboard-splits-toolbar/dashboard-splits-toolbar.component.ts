import { ChangeDetectionStrategy, Component, OnInit } from '@angular/core';
import { filter } from 'rxjs';
import { StoreDispatcher } from '@shared/base-classes/store-dispatcher.class';
import { selectLoadingStateLength } from '@app/layout/toolbar/loading.reducer';
import { selectDashboardSplitsPeersAndLinksAndSetsAndFetching } from '@network/splits/dashboard-splits.state';
import { DashboardSplitsGetSplits } from '@network/splits/dashboard-splits.actions';

@Component({
  selector: 'mina-dashboard-splits-toolbar',
  templateUrl: './dashboard-splits-toolbar.component.html',
  styleUrls: ['./dashboard-splits-toolbar.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'h-xl' },
  standalone: false,
})
export class DashboardSplitsToolbarComponent
  extends StoreDispatcher
  implements OnInit
{
  /** Peers connected to the selected node, from its last answer. */
  connected: number;
  fetching: boolean;
  nodeName: string = '';
  loaded: boolean = false;

  ngOnInit(): void {
    this.select(
      selectLoadingStateLength,
      (length: number) => {
        this.fetching = length > 0;
        this.detect();
      },
      filter((length: number) => this.fetching !== length > 0),
    );
    this.select(
      selectDashboardSplitsPeersAndLinksAndSetsAndFetching,
      ({ peers, fetching, node }) => {
        this.loaded = !fetching;
        this.nodeName = node ?? '';
        this.connected = peers.filter(p => !p.self).length;
        this.detect();
      },
    );
  }

  refresh(): void {
    this.dispatch(DashboardSplitsGetSplits);
  }
}
