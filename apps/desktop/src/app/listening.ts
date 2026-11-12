/**
 * Not listening (ADR 0057), as the desktop says it: the words for the status card, the
 * switch and Settings, and the choices for how long. Plain functions, so the tests run
 * them without React.
 */
import type { NotListening, Status } from "@ghost/client-ts";

/** The choices when turning listening off, like do-not-disturb's. */
export const OFF_FOR: { label: string; minutes?: number }[] = [
  { label: "For 30 minutes", minutes: 30 },
  { label: "For an hour", minutes: 60 },
  { label: "Until I turn it back on" },
];

/** The app's own shortcut for the switch: off until turned back on, or on again. */
export const LISTENING_SHORTCUT = "mod+shift+m";

/** Whether the status says Ghost's microphone is closed. */
export function notListening(status: Pick<Status, "not_listening"> | null | undefined): NotListening | null {
  return status?.not_listening ?? null;
}

/** "3:30 PM", or "tomorrow 9:00 AM" when it ends on another day than `now`. */
export function untilText(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  const time = at.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
  const sameDay = at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate();
  return sameDay ? time : `tomorrow ${time}`;
}

/** The status card's second line while not listening. */
export function notListeningSub(off: NotListening, now: Date = new Date()): string {
  return off.until ? `Mic off until ${untilText(off.until, now)}` : "Mic off";
}

/** One sentence for Settings and tooltips: what is true now. */
export function listeningSentence(off: NotListening | null, now: Date = new Date()): string {
  if (!off) return "Ghost listens for its name. Nothing you say leaves this computer until you say it.";
  const until = off.until ? `until ${untilText(off.until, now)}` : "until you turn it back on";
  return `The microphone is closed ${until}. Ghost hears nothing, not even its name. Typing, your phone, schedules and watches still work.`;
}

/** The switch's accessible name and tooltip. */
export function switchLabel(off: NotListening | null): string {
  return off ? "Listen again" : "Stop listening";
}
