import * as React from "react";

import {
  NodeClientProvider,
  NodeStatusProvider,
} from "@minanode/node-client/react";

import { BlockProducerPage } from "../pages/block-producer";
import { BootPage } from "../pages/boot";
import { AccountsPage } from "../pages/accounts";
import { BlockPage } from "../pages/block";
import { BlocksPage } from "../pages/blocks";
import { PoolPage } from "../pages/pool";
import { KeyPage } from "../pages/key";
import { SendPage } from "../pages/send";
import { SnarkWorkerPage } from "../pages/snark-worker";
import { NotFoundPage } from "../pages/not-found";
import { OverviewPage } from "../pages/overview";
import { PeersPage } from "../pages/peers";
import { autostart, startNode, useBoot } from "./boot-context";
import { BootScreen } from "./boot-screen";
import { matchRoute, usePathname, type Route } from "./router";
import { Shell } from "./shell";
import { StartScreen } from "./start-screen";

const titles: Record<Route["page"], string> = {
  overview: "Overview",
  blocks: "Blocks",
  block: "Block",
  accounts: "Accounts",
  pool: "Transaction pool",
  peers: "Peers",
  boot: "Start-up",
  key: "Your key",
  send: "Send",
  worker: "SNARK worker",
  producer: "Block producer",
  "not-found": "Not found",
};

function Page({ route }: { route: Route }) {
  switch (route.page) {
    case "overview":
      return <OverviewPage />;
    case "blocks":
      return <BlocksPage />;
    case "block":
      return <BlockPage id={route.id} />;
    case "accounts":
      return <AccountsPage publicKey={route.publicKey} />;
    case "pool":
      return <PoolPage />;
    case "peers":
      return <PeersPage />;
    case "boot":
      return <BootPage />;
    case "key":
      return <KeyPage />;
    case "send":
      return <SendPage />;
    case "worker":
      return <SnarkWorkerPage />;
    case "producer":
      return <BlockProducerPage />;
    case "not-found":
      return <NotFoundPage path={route.path} />;
  }
}

export function App() {
  const boot = useBoot();
  // Nothing downloads until the visitor starts the node, or the page
  // reloaded to start it again (boot/autostart.ts).
  const [started, setStarted] = React.useState(autostart);
  const clicked = React.useRef(false);
  React.useEffect(() => {
    if (!started) return;
    // For the headless checks: past the start screen, before the first step.
    document.documentElement.dataset.boot = "starting";
    startNode();
    // Start's button is gone: focus the boot screen's heading instead.
    if (clicked.current) {
      clicked.current = false;
      document.getElementById("page-title")?.focus({ preventScroll: true });
    }
  }, [started]);
  const pathname = usePathname();
  const route = matchRoute(pathname);
  const [opened, setOpened] = React.useState(false);
  const inShell = boot.client !== null && (boot.synced || opened);

  React.useEffect(() => {
    if (inShell) document.title = `${titles[route.page]} · Web node · minanode`;
  }, [inShell, route.page]);
  // Move focus to the new page's heading after a navigation, so a screen
  // reader announces it.
  const shown = React.useRef(pathname);
  React.useEffect(() => {
    if (shown.current === pathname) return;
    shown.current = pathname;
    document.getElementById("page-title")?.focus({ preventScroll: true });
  }, [pathname]);

  if (!started) {
    return (
      <StartScreen
        role={boot.role}
        onStart={() => {
          clicked.current = true;
          setStarted(true);
        }}
      />
    );
  }
  if (!inShell || !boot.client) {
    return <BootScreen boot={boot} onOpen={() => setOpened(true)} />;
  }
  return (
    <NodeClientProvider client={boot.client}>
      <NodeStatusProvider>
        <Shell route={route}>
          <Page route={route} />
        </Shell>
      </NodeStatusProvider>
    </NodeClientProvider>
  );
}
