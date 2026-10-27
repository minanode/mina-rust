import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { createNodeClient } from "@minanode/node-client";
import { NodeClientProvider } from "@minanode/node-client/react";

import { App } from "./app/app";
import "./app.css";

// The node that serves the dashboard: its own origin. Another app can point
// the same client at any node with `baseUrl`.
const client = createNodeClient();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <NodeClientProvider client={client}>
      <App />
    </NodeClientProvider>
  </StrictMode>,
);
