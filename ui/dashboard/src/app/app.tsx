import * as React from "react";

import { AccountPage } from "../pages/account";
import { BlockPage } from "../pages/block";
import { BlocksPage } from "../pages/blocks";
import { GraphqlPage } from "../pages/graphql";
import { MempoolPage } from "../pages/mempool";
import { NodeStatusPage } from "../pages/node-status";
import { NotFoundPage } from "../pages/not-found";
import { OverviewPage } from "../pages/overview";
import { PeersPage } from "../pages/peers";
import { ProducerPage } from "../pages/producer";
import { ReportPage } from "../pages/report";
import { WorkerPage } from "../pages/worker";
import { NodeStatusProvider } from "@minanode/node-client/react";
import { matchRoute, usePathname, type Route } from "./router";
import { Shell } from "./shell";

const titles: Record<Route["page"], string> = {
  overview: "Overview",
  worker: "SNARK worker",
  producer: "Block producer",
  blocks: "Blocks",
  block: "Block",
  account: "Accounts",
  mempool: "Mempool",
  peers: "Peers",
  status: "Status",
  graphql: "GraphQL",
  report: "Report a problem",
  "not-found": "Not found",
};

function Page({ route }: { route: Route }) {
  switch (route.page) {
    case "overview":
      return <OverviewPage />;
    case "worker":
      return <WorkerPage />;
    case "producer":
      return <ProducerPage />;
    case "blocks":
      return <BlocksPage />;
    case "block":
      return <BlockPage id={route.id} />;
    case "account":
      return <AccountPage publicKey={route.publicKey} />;
    case "mempool":
      return <MempoolPage />;
    case "peers":
      return <PeersPage />;
    case "status":
      return <NodeStatusPage />;
    case "graphql":
      return <GraphqlPage />;
    case "report":
      return <ReportPage />;
    case "not-found":
      return <NotFoundPage path={route.path} />;
  }
}

export function App() {
  const pathname = usePathname();
  const route = matchRoute(pathname);
  const shown = React.useRef(pathname);

  React.useEffect(() => {
    document.title = `${titles[route.page]} · Node dashboard`;
    // After an in-app navigation, move focus to the new page's heading so
    // a screen reader announces it; not on the first load (nor when React
    // runs the effect again for the same path).
    if (shown.current === pathname) return;
    shown.current = pathname;
    document.getElementById("page-title")?.focus({ preventScroll: true });
  }, [pathname, route.page]);

  return (
    <NodeStatusProvider>
      <Shell route={route}>
        <Page route={route} />
      </Shell>
    </NodeStatusProvider>
  );
}
