import { ChangeDetectionStrategy, Component } from '@angular/core';
import { MinaTableRustWrapper } from '@shared/base-classes/mina-table-rust-wrapper.class';
import {
  DashboardPeer,
  DashboardPeerStatus,
} from '@shared/types/dashboard/dashboard.peer';
import { TableColumnList } from '@mina-rust/shared';
import { DashboardPeersSort } from '@dashboard/dashboard.actions';
import {
  selectDashboardPeers,
  selectDashboardPeersSort,
} from '@dashboard/dashboard.state';
import { filter } from 'rxjs';
import { formatHeight } from '@shared/helpers/freshness.helper';

@Component({
  selector: 'mina-dashboard-peers-minimal-table',
  templateUrl: './dashboard-peers-minimal-table.component.html',
  styleUrls: ['./dashboard-peers-minimal-table.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'flex-column h-100' },
  standalone: false,
})
export class DashboardPeersMinimalTableComponent extends MinaTableRustWrapper<DashboardPeer> {
  protected readonly DashboardPeerStatus = DashboardPeerStatus;
  protected readonly formatHeight = formatHeight;
  protected readonly tableHeads: TableColumnList<DashboardPeer> = [
    { name: 'status', sort: 'status' },
    // When the peer entered that status (connected since, and so on).
    { name: 'since (UTC)', sort: 'timestamp' },
    // The tip the peer last advertised to this node (R27 #10).
    { name: 'peer tip', sort: 'height' },
  ];

  override async ngOnInit(): Promise<void> {
    await super.ngOnInit();
    this.listenToPeersChanges();
  }

  protected override setupTable(): void {
    this.table.gridTemplateColumns = [130, 165, 89];
    this.table.minWidth = 384;
    this.table.sortClz = DashboardPeersSort;
    this.table.sortSelector = selectDashboardPeersSort;
    this.table.trackByFn = (_: number, row: DashboardPeer) =>
      row.peerId + row.status + row.bestTip + row.timestamp;
  }

  private listenToPeersChanges(): void {
    this.select(selectDashboardPeers, (peers: DashboardPeer[]) => {
      this.table.rows = peers;
      this.table.detect();
    });
  }

  protected override onRowClick(row: DashboardPeer): void {}
}
