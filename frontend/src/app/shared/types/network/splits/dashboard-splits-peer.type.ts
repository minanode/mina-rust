export class DashboardSplitsPeer {
  address: string;
  node: string;
  /** The node whose peers are shown, in the middle of the graph. */
  self?: boolean;
  peerId?: string;
  radius?: number;
  outgoingConnections?: number;
  incomingConnections?: number;
}
