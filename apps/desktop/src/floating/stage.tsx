/**
 * Ghost on stage (`stage.html`, ADR 0059 as amended 2026-11-13): the floating room in an
 * ordinary window on its own backdrop, opened from the tray or Settings, for showing Ghost to
 * people. A meeting's "share a window" picks it by its title, "Ghost · Stage". It reads the
 * daemon as the main window does; `?mock=<scenario>` swaps in the in-memory daemon in
 * development (`stage.html?mock=floating` plays the whole sequence on the stage).
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
      <FloatingRoom stage />
    </GhostProvider>,
  );
}

void boot();
