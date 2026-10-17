import { GhostClient } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import { makeTransport } from "./transport";

makeTransport().then((transport) => {
  const client = new GhostClient(transport);
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <GhostProvider client={client}>
        <App />
      </GhostProvider>
    </StrictMode>,
  );
});
