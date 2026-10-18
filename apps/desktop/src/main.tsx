import "@fontsource-variable/inter/opsz.css";
import "@fontsource-variable/jetbrains-mono";
import "./ui/tokens.css";
import "./ui/base.css";
import "./ui/components.css";

import { GhostClient, type Transport } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./app/App";
import { type DesignState, DesignProvider } from "./design/context";
import { makeTransport } from "./transport";

async function boot() {
  let transport: Transport;
  let design: DesignState | null = null;

  // Design mode: ?mock=<scenario> swaps the daemon for an in-memory one. The branch is
  // dead code in production builds (import.meta.env.DEV is false), so the mock layer and
  // its fixtures are never bundled there.
  const scenario = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("mock") : null;
  if (import.meta.env.DEV && scenario) {
    const mock = await import("./design/mock");
    ({ transport, design } = mock.createMock(scenario));
  } else {
    transport = await makeTransport();
  }

  const client = new GhostClient(transport);
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <DesignProvider value={design}>
        <GhostProvider client={client}>
          <App />
        </GhostProvider>
      </DesignProvider>
    </StrictMode>,
  );
}

void boot();
