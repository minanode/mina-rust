import { Injectable } from '@angular/core';
import { map, Observable } from 'rxjs';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { DashboardSplits } from '@shared/types/network/splits/dashboard-splits.type';
import { RustService } from '@core/services/rust.service';
import { observedTopology, PeerRecord } from '@shared/helpers/topology.helper';

/**
 * Network > Topology: the peers the selected node is connected to now, from
 * its own `/state/peers` (R28). It used to draw 100 randomly generated peers
 * and simulate splits and merges on them; that sample data is gone.
 */
@Injectable({ providedIn: 'root' })
export class DashboardSplitsService {
  constructor(private rust: RustService) {}

  getPeers(node: MinaNode): Observable<DashboardSplits & { node: string }> {
    return this.rust.get<PeerRecord[]>('/state/peers', node).pipe(
      map(records => ({
        ...observedTopology(records, node.name),
        node: node.name,
      })),
    );
  }
}
