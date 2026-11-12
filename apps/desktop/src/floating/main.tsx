/**
 * The floating room's page (`floating.html`, ADR 0059), loaded by the surface process
 * (`heka-ghost-desktop --floating`). It reads the daemon through the same bridge as the main
 * window; `?mock=<scenario>` swaps in the in-memory daemon in development, as main.tsx does
 * (`?mock=floating` plays a whole spoken session over and over).
 */
import "@fontsource-variable/inter/opsz.css";
import "@fontsource-variable/jetbrains-mono";
import "./floating.css";

import { GhostClient, type Transport } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { createRoot } from "react-dom/client";

import { makeTransport } from "../transport";
import { FloatingRoom } from "./Floating";

async function boot() {
  let transport: Transport;
  const scenario = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("mock") : null;
  if (import.meta.env.DEV && scenario) {
    const mock = await import("../design/mock");
    ({ transport } = mock.createMock(scenario));
  } else {
    transport = await makeTransport();
  }
  createRoot(document.getElementById("root")!).render(
    <GhostProvider client={new GhostClient(transport)}>
      <FloatingRoom />
    </GhostProvider>,
  );
}

void boot();
