/**
 * Owner: tray. The tray's own reads: the last change (for Undo), the microphone's name and
 * the line names. All real client calls; nothing here writes.
 */
import type { AuditEntry, ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useState } from "react";

/** The gate's undo window (ghost_core.models.UNDO_WINDOW). Undo is offered only inside it. */
export const UNDO_WINDOW_MS = 10 * 60_000;

/**
 * The newest thing Ghost actually did (executed, or failed trying), kept across a drop.
 * Re-read shortly after any event: the daemon sends a change's events in one burst (pending,
 * status, outcome), React keeps only the last, so no single event type can be relied on.
 */
export function useLastChange(): {
  entry: AuditEntry | null;
  /** When `entry` is an undo, the change it reversed (so the tray can say what was undone). */
  undone: AuditEntry | null;
  loaded: boolean;
  refresh: () => void;
} {
  const { client, connected, lastEvent } = useGhost();
  const [entry, setEntry] = useState<AuditEntry | null>(null);
  const [undone, setUndone] = useState<AuditEntry | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [nonce, setNonce] = useState(0);
  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  useEffect(() => {
    if (!connected) return;
    let live = true;
    const t = window.setTimeout(() => {
      client
        .audit({ limit: 20 })
        .then((page) => {
          if (!live) return;
          const e = page.items.find((x) => x.status === "executed" || x.status === "failed" || x.status === "undone") ?? null;
          setEntry(e);
          setUndone(e?.undo_of ? (page.items.find((x) => x.id === e.undo_of) ?? null) : null);
          setLoaded(true);
        })
        .catch(() => live && setLoaded(true));
    }, loaded ? 250 : 0);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, connected, lastEvent, nonce]);
  return { entry, undone, loaded, refresh };
}

/** Executed by Ghost, not itself an undo, and still inside the gate's undo window. */
export function canUndo(e: AuditEntry | null, now = Date.now()): boolean {
  if (!e || e.status !== "executed" || e.undo_of) return false;
  return now - Date.parse(e.created_at) < UNDO_WINDOW_MS;
}

/**
 * What an undo record reversed, in the gate's words without the "Undo:" prefix: the original
 * entry's summary when it is on the same page, else the undo's own summary stripped.
 */
export function undoneSummary(entry: AuditEntry, undone: AuditEntry | null): string {
  return undone?.summary ?? entry.summary.replace(/^Und(?:o|id):\s*/i, "");
}

/**
 * A short phrase for a change, for places with one line of room (the native menu): "close
 * app 151", "approved app 208". Built from the summary's verb and the target's short form.
 */
export function shortChange(summary: string, target: string | null | undefined): string {
  const body = summary.replace(/^Und(?:o|id):\s*/i, "").replace(/^On [^:]+:\s*/, "");
  const verb = body.split(/[,:]/)[0].replace(/\s+the issue$/, "").trim();
  return target ? `${verb} ${shortTarget(target)}` : verb;
}

/** "ghost-mock/app#151" -> "app 151". */
export function shortTarget(target: string): string {
  const m = /([^/]+)#(\d+)$/.exec(target.replace(/^github:/, ""));
  return m ? `${m[1]} ${m[2]}` : target;
}

/**
 * Device ids the operator knows by another name: what the device is to them. One clause
 * each, so the row's description never clips at the popover's 360.
 */
const MIC_NAMES: Record<string, string> = {
  "ghost.mic": "Ghost microphone",
  default: "System default microphone",
  pulse: "System default microphone",
  pipewire: "System default microphone",
};

/** The device id as people say it: "default" is the system's own choice, ghost.mic is Ghost's own. */
export function micLabel(name: string | null): string {
  return name ? (MIC_NAMES[name] ?? name) : "System default microphone";
}

/** What the operator set up behind a named device, for the row's tooltip (ghost.mic is the echo-canceled source). */
const MIC_NOTES: Record<string, string> = { "ghost.mic": "ghost.mic · echo canceled" };

/** The tooltip for the Mute row: the note for a known device, the raw id when that is what shows. */
export function micTitle(name: string | null): string | undefined {
  if (!name) return undefined;
  return MIC_NOTES[name] ?? (micLabel(name) === name ? name : undefined);
}

/** Past forms for the verbs the gate's summaries start with, for the line that says it happened. */
const PAST: Record<string, string> = {
  add: "Added", approve: "Approved", assign: "Assigned", close: "Closed", comment: "Commented", create: "Created",
  delete: "Deleted", edit: "Edited", label: "Labeled", lock: "Locked", merge: "Merged", pin: "Pinned", remove: "Removed",
  rename: "Renamed", reopen: "Reopened", request: "Requested", set: "Set", start: "Started", transfer: "Transferred",
  unassign: "Unassigned", unlabel: "Unlabeled", unlock: "Unlocked", unpin: "Unpinned", update: "Updated",
};

/**
 * What a confirmed change did, in one short line: "Closed app 151". Falls back to "Done:
 * close app 151" for a verb it doesn't know, rather than guessing at English.
 */
export function doneChange(summary: string, target: string | null | undefined): string {
  const s = shortChange(summary, target);
  const [w, ...rest] = s.split(" ");
  const past = PAST[w.toLowerCase()] ?? (/ed$/i.test(w) ? w[0].toUpperCase() + w.slice(1) : null);
  return past ? [past, ...rest].join(" ") : `Done: ${s}`;
}

/** "close app 151" -> "Close app 151": a phrase that starts a menu item. */
export function capitalize(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
}

/** The microphone Ghost listens on: the configured input, or the system default. */
export function useMicName(): string | null {
  const { client, connected } = useGhost();
  const [name, setName] = useState<string | null>(null);
  useEffect(() => {
    if (!connected) return;
    let live = true;
    client
      .devices()
      .then((d) => {
        if (!live) return;
        const dflt = d.devices.find((x) => x.is_default_input);
        setName(d.input_device ?? dflt?.name ?? null);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, connected]);
  return name;
}

/** Line names, so "On the Accounting line" uses the name the operator gave it. */
export function useLines(): ConversationView[] {
  const { client, connected, conversationsVersion } = useGhost();
  const [lines, setLines] = useState<ConversationView[]>([]);
  useEffect(() => {
    if (!connected) return;
    let live = true;
    client
      .conversations()
      .then((l) => live && setLines(l))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [client, connected, conversationsVersion]);
  return lines;
}
