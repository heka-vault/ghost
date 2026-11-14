/**
 * The hero's float: the desktop app's own floating room (apps/desktop/src/floating, ADR 0059)
 * driven by its in-memory mock daemon's `floating-site` scenario, one turn on the clock of the
 * site's sound (site/audio/turn.json). Nothing here is drawn for the site: if the app's floating
 * room changes, so does this. Synthetic words only, no network. The page's own address carries
 * the room's placement (`?placement=centre` on a phone), which the room reads as the shell's word.
 *
 * It tells the front page each time the turn starts (`ghost:loop`), so the sound stays in step,
 * and starts the turn again from the wake when the front page asks (`ghost:restart`, as the
 * visitor turns the sound on).
 */
import "../../../apps/desktop/src/floating/floating.css";

import { GhostClient } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { useEffect, useMemo, useState } from "react";

import { createMock } from "../../../apps/desktop/src/design/mock";
import { FloatingRoom } from "../../../apps/desktop/src/floating/Floating";

const tellFront = (ghost: string) => {
  if (parent !== window) parent.postMessage({ ghost }, location.origin);
};

export default function FloatDemo() {
  const [run, setRun] = useState(0);
  const mock = useMemo(() => createMock("floating-site", { onSiteLoop: () => tellFront("loop") }), [run]);
  const client = useMemo(() => new GhostClient(mock.transport), [mock]);
  useEffect(() => () => mock.dispose(), [mock]);
  useEffect(() => {
    const restart = (e: MessageEvent) => {
      if (e.origin === location.origin && e.source === parent && e.data?.ghost === "restart") setRun((n) => n + 1);
    };
    addEventListener("message", restart);
    return () => removeEventListener("message", restart);
  }, []);
  return (
    <GhostProvider key={run} client={client}>
      <FloatingRoom />
    </GhostProvider>
  );
}
