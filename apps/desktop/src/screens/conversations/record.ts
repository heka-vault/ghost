/**
 * Owner: conversations. An agent's stored transcript as the conversation screen's record (one
 * transcript component, two places: docs/design/overhaul-2026-11.md step 4). The stored turns go
 * through the session's own `buildHistory`; the agent speaks in its own mark, not Ghost's; a
 * stored tool call (`role: "call"`) becomes a work row, unless the session's kept activity
 * (ADR 0058) already has its calls. Pure and tested.
 */
import type { Turn, TurnView } from "@ghost/client-ts";
import { type Activity, appOf } from "@ghost/ui";

import { activityItems, type ExtraItem, weave, withGaps } from "../session/timeline";
import { buildHistory, type Item, type LineInfo } from "../session/transcript";

const CALL = /^([A-Za-z0-9_.-]+)\s*\(/;

/** A stored turn as the live record reads one. A stored turn doesn't say how it was said; it reads as spoken. */
export function asTurn(t: TurnView): Turn {
  return { at: t.at, role: t.role === "call" ? "system" : t.role, session_id: t.session_id, text: t.text, via: "voice" } as Turn;
}

export function agentRecord(turns: readonly TurnView[], slug: string, lines: LineInfo[], activity: readonly Activity[], apps: readonly string[]): Item[] {
  const said = turns.filter((t) => t.role !== "call");
  const withActivity = new Set(activity.filter((a) => a.kind === "call").map((a) => a.session_id));
  // Calls the store kept as turns, where no activity was kept for that session.
  const stored: ExtraItem[] = turns
    .filter((t) => t.role === "call" && !withActivity.has(t.session_id))
    .map((t, i) => {
      const name = CALL.exec(t.text)?.[1] ?? t.text.split(/\s/)[0];
      const app = appOf(name, apps);
      return {
        kind: "work",
        key: `wc-${t.at}-${i}`,
        at: t.at,
        calls: [{ id: `c-${t.id}`, name, app, tool: (app ? name.slice(app.length + 1) : name).replace(/[_.]/g, " "), startedAt: t.at, endedAt: t.at }],
        handover: null,
      };
    });
  const items = buildHistory(said.map(asTurn), lines).map((it) => (it.kind === "ghost" ? { ...it, slug } : it));
  return withGaps(weave(items, [...activityItems(activity, said.map(asTurn), apps), ...stored]));
}
