/**
 * The floating room's switch on the rail (ADR 0059 as amended 2026-11-14): where the room's
 * button was, the answer to “I want Ghost big and present”. Pressed while Ghost floats over the
 * desktop when called; its tooltip says what that means in one line. Where this desktop can't
 * float there is no switch to press: the sentence saying why is in Settings › General, beside
 * the switch it explains, never a greyed button here.
 */
import { PictureInPicture2 } from "lucide-react";

import { IconButton } from "../ui";
import { useFloating } from "./floating";

/** The rail's words for the switch, from the shell's state. Null: no switch (can't float, or not known yet). */
export function floatingSwitchLabel(st: ReturnType<typeof useFloating>["status"]): string | null {
  if (!st || st.support.kind === "no") return null;
  return st.settings.on
    ? "Ghost floats over your work when you call it. Click to keep it in this window."
    : "Float over the desktop: when you call Ghost, it comes forward over your work.";
}

export function FloatingSwitch() {
  const floating = useFloating();
  const st = floating.status;
  const label = floatingSwitchLabel(st);
  if (!st || !label) return null;
  return (
    <IconButton
      className="rail__float"
      label={label}
      icon={<PictureInPicture2 />}
      pressed={st.settings.on}
      aria-pressed={st.settings.on}
      tooltipSide="right"
      onClick={() => void floating.set({ ...st.settings, on: !st.settings.on })}
    />
  );
}
