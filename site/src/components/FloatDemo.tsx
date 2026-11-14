/**
 * The hero's float: the desktop app's own floating room (apps/desktop/src/floating, ADR 0059)
 * driven by its in-memory mock daemon's `floating-site` scenario, a short loop of the README's
 * tour. Nothing here is drawn for the site: if the app's floating room changes, so does this.
 * Synthetic words only, no network. The page's own address carries the room's placement
 * (`?placement=centre` on a phone), which the room reads as the shell's word.
 */
import "../../../apps/desktop/src/floating/floating.css";

import { GhostClient } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { useState } from "react";

import { createMock } from "../../../apps/desktop/src/design/mock";
import { FloatingRoom } from "../../../apps/desktop/src/floating/Floating";

export default function FloatDemo() {
  const [client] = useState(() => new GhostClient(createMock("floating-site").transport));
  return (
    <GhostProvider client={client}>
      <FloatingRoom />
    </GhostProvider>
  );
}
