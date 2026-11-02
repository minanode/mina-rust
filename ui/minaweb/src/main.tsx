// First: the start gate takes the address's nonce before anything reads it.
import "./app/load-gate";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./app/app";
import "./app.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
