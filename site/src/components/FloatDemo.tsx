/**
 * The hero's float: the desktop app's own floating room (apps/desktop/src/floating, ADR 0059)
 * driven by its in-memory mock daemon's `floating` scenario, the same sequence
 * `floating.html?mock=floating` plays in development. Nothing here is drawn for the site: if the
 * app's floating room changes, so does this. Synthetic words only, no network.
 */
import "../../../apps/desktop/src/floating/floating.css";

import { GhostClient } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { useState } from "react";

import { createMock } from "../../../apps/desktop/src/design/mock";
import { FloatingRoom } from "../../../apps/desktop/src/floating/Floating";

export default function FloatDemo() {
  // The mock's requests answer at once; its own latency is only for the app's loading states.
  const [client] = useState(() => new GhostClient(createMock("floating").transport));
  return (
    <GhostProvider client={client}>
      <FloatingRoom />
    </GhostProvider>
  );
}
