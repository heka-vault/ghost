/**
 * The waiting list's words and checks (ADR 0029, ADR 0051), shared by Today, the room and the
 * tray: where an item came from, when a question stops waiting, whether an answer may be sent,
 * and what an action's answer means. Moved here from the retired What needs you screen.
 */
import type { QuestionView, WaitingItemView } from "@ghost/client-ts";

import { agentName } from "../../ui";
import { toolTitle } from "../settings/tools";

/**
 * Where an item came from, in plain words: a watch, a webhook, a change, an agent, an outside
 * tool, an app, Ghost itself. An outside tool says it is one (ADR 0048), and what an agent
 * did on its own says so too (ADR 0047): its scheduled run, its run for a watch, or a
 * question it asked from one, whose answer starts one more run.
 */
export function sourceLabel(item: WaitingItemView): string {
  const [kind, rest] = item.source.includes(":") ? item.source.split(/:(.*)/s, 2) : [item.source, ""];
  if (kind === "watch") return `Watch ${rest}`;
  if (kind === "hook") return `Webhook ${rest}`;
  if (kind === "gate") return "A change";
  const q = item.question && item.question.kind === "question" ? item.question : null;
  if (kind === "client") return q ? `Asked by ${q.asked_by}, an outside tool` : `From ${toolTitle(rest)}, an outside tool`;
  if (q) return q.in_run ? `Asked by ${q.asked_by} in a scheduled run` : `Asked by ${q.asked_by}`;
  if (kind === "agent") {
    const who = agentName(rest);
    // A run's answer is kept under `agent:<slug>:schedule:<id>` or `agent:<slug>:act:<hash>`.
    const after = item.key.startsWith(`${item.source}:`) ? item.key.slice(item.source.length + 1) : "";
    if (after.startsWith("schedule:")) return `${who}’s scheduled run`;
    if (after.startsWith("act:")) return `${who}’s run for a watch`;
    return `Agent ${who}`;
  }
  if (kind === "app") return `App ${rest}`;
  return "Ghost";
}

/** When an open question stops waiting, in a few words: "expires in 3 h", "expires in 20 min". */
export function expiresIn(q: QuestionView, now: number = Date.now()): string {
  const mins = Math.round((new Date(q.expires_at).getTime() - now) / 60000);
  if (mins <= 0) return "expiring now";
  if (mins < 60) return `expires in ${mins} min`;
  const hours = Math.round(mins / 60);
  if (hours < 48) return `expires in ${hours} h`;
  return `expires in ${Math.round(hours / 24)} days`;
}

/** Whether a typed answer may be sent: any words, unless only the choices will do. */
export function canSend(q: QuestionView, text: string): boolean {
  const t = text.trim();
  if (!t) return false;
  if (!q.choices_only) return true;
  return q.choices.some((c) => c.toLowerCase() === t.toLowerCase());
}

/**
 * What an action answered, when it didn't do what was asked. The daemon answers 200 with
 * `ok: false` (an item already gone, a change no longer waiting) or, for a confirmed change,
 * an outcome whose status is `failed` or `partial`: those are failures to show, not "Done".
 */
export function actionFailure(r: unknown): { title: string; text: string; tone: "danger" | "warn" } | null {
  if (!r || typeof r !== "object") return null;
  const v = r as { ok?: boolean; detail?: string | null; status?: string; error?: string | null; summary?: string };
  if (v.ok === false) return { title: "That didn’t work", text: v.detail || "It may have changed meanwhile; the list is up to date now.", tone: "danger" };
  if (v.status === "failed") return { title: "The change didn’t go through", text: v.error || v.summary || "", tone: "danger" };
  if (v.status === "partial") return { title: "Only part of the change went through", text: v.error || v.summary || "", tone: "warn" };
  return null;
}

/** Minutes until 9:00 tomorrow, for "Tomorrow morning". */
export function untilTomorrowMorning(): number {
  const t = new Date();
  t.setDate(t.getDate() + 1);
  t.setHours(9, 0, 0, 0);
  return Math.max(1, Math.round((t.getTime() - Date.now()) / 60000));
}

