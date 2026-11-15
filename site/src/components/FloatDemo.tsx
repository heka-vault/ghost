/**
 * The hero's float: the desktop app's own floating view (apps/desktop/src/floating, ADR 0059)
 * driven by the site's own small mock (apps/desktop/src/design/mock/site.ts), the `floating-site`
 * turn on the clock of the site's sound (site/audio/turn.json). The desktop's `?mock=` has the
 * whole mock daemon; a test holds the two to the same scene and clock, so nothing here is drawn
 * for the site: if the app's floating view changes, so does this. Synthetic words only, no
 * network. The page's own address carries the room's placement (`?placement=centre` on a
 * phone), which the room reads as the shell's word.
 *
 * It tells the front page each time the turn starts (`ghost:loop`), so the sound stays in step,
 * starts the turn again from the wake when the front page asks (`ghost:restart`, as the visitor
 * turns the sound on or comes back), and settles when asked (`ghost:rest`, after two turns):
 * the scene stops, every timer with it, and Wisp waits where it comes forward, lit and still.
 * Nothing moves at rest, so it costs next to nothing to keep on screen.
 */
import "../../../apps/desktop/src/floating/floating.css";

import { GhostClient } from "@ghost/client-ts";
import { GhostProvider } from "@ghost/ui";
import { useEffect, useMemo, useState } from "react";

import { createSiteMock } from "../../../apps/desktop/src/design/mock/site";
import { FloatingRoom } from "../../../apps/desktop/src/floating/Floating";
import { layoutOf } from "../../../apps/desktop/src/floating/layout";
import { Mascot } from "../../../apps/desktop/src/ui/Mascot";

const tellFront = (ghost: string) => {
  if (parent !== window) parent.postMessage({ ghost }, location.origin);
};

export default function FloatDemo() {
  const [run, setRun] = useState(0);
  const [resting, setResting] = useState(false);
  useEffect(() => {
    const told = (e: MessageEvent) => {
      if (e.origin !== location.origin || e.source !== parent) return;
      if (e.data?.ghost === "restart") {
        setResting(false);
        setRun((n) => n + 1);
      }
      if (e.data?.ghost === "rest") setResting(true);
    };
    addEventListener("message", told);
    return () => removeEventListener("message", told);
  }, []);
  return resting ? <AtRest /> : <Playing key={run} />;
}

function Playing() {
  const mock = useMemo(() => createSiteMock(() => tellFront("loop")), []);
  const client = useMemo(() => new GhostClient(mock.transport), [mock]);
  useEffect(() => () => mock.dispose(), [mock]);
  return (
    <GhostProvider client={client}>
      <FloatingRoom />
    </GhostProvider>
  );
}

/**
 * Ghost at rest after its turns: Wisp where it comes forward, lit, drawn once (the floating
 * room's own resting look, `flt-rest`, as on its stage). No frame loop, no field, no timers.
 */
function AtRest() {
  const placement = new URLSearchParams(location.search).get("placement") === "centre" ? "centre" : "beside";
  const g = layoutOf({ w: innerWidth, h: innerHeight, placement, side: null, places: 0, card: false, captions: false });
  return (
    <div className="flt-root" data-scene="line" data-shown="0" data-placement={placement} data-site-rest="1" style={{ ["--flt-s" as string]: g.scale.toFixed(3), ["--flt-wisp" as string]: `${g.wisp}px` }}>
      {/* Centred on the core, where Wisp stood while forward (the stage's own sits higher, over its words). */}
      <div className="flt-rest" aria-hidden style={{ left: g.core[0], top: g.core[1], translate: "-50% -50%" }}>
        <div className="flt-rest__wisp">
          <Mascot size={g.wisp} still lit="rgb(89 217 255)" />
        </div>
      </div>
    </div>
  );
}
