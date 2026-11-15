/**
 * Now: the head of Today's timeline. What needs the operator, by what they do about it (Decide,
 * Answer, Know), each with its one action in reach; then the agents at work right now, with
 * live clocks and a way to stop a run. Every action the retired What needs you screen had is
 * here: Confirm and Cancel (the click is confirmation evidence, ADR 0013), an answer in the
 * operator's own words or a choice, Decline (ADR 0051), Got it, Later and Open (ADR 0029).
 *
 * Titles, questions and choices are other systems' and agents' words: shown as text only.
 */
import type { PendingProposal, QuestionView, WaitingItemView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { BellOff, ExternalLink } from "lucide-react";
import { useEffect, useState } from "react";

import { placeOf } from "../../app/places";
import { hrefFor } from "../../app/router";
import { nameOf, useToday } from "../../app/today";
import { Button, IconButton, Input, LineGlyph, Menu, PendingChange, alwaysDone, cx, errorText, relativeTime, useLive, useToast } from "../../ui";
import { appWords } from "../../ui/always";
import { needLine, runReason, took } from "./model";
import { liveNeeds, withoutLive } from "./live";
import { actionFailure, canSend, expiresIn, sourceLabel, untilTomorrowMorning } from "./needs";
import { count, tallyFor } from "./telemetry";

type Done = string | { title: string; description: string };
type Act = (key: string, run: () => Promise<unknown>, done?: Done) => Promise<void>;

function useAct(): { busy: string | null; act: Act } {
  const toast = useToast();
  const { reload } = useToday();
  const [busy, setBusy] = useState<string | null>(null);
  const act: Act = async (key, run, done) => {
    setBusy(key);
    try {
      const failed = actionFailure(await run());
      if (failed) toast({ title: failed.title, description: failed.text || undefined, tone: failed.tone });
      else if (done) toast(typeof done === "string" ? { title: done } : done);
      reload();
    } catch (e) {
      toast({ title: "That didn’t work", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  return { busy, act };
}


/** `pending` and `question`: what the talk bar's live strip shows right now, left out here so nothing shows twice. */
export function Now({ pending = null, question = null }: { pending?: PendingProposal | null; question?: QuestionView | null } = {}) {
  const today = useToday();
  const { client } = useGhost();
  const { online } = useLive();
  const { busy, act } = useAct();
  const { decide, answer, know } = withoutLive(liveNeeds(today.needs, pending), pending, question);
  const clear = !decide.length && !answer.length && !know.length;

  return (
    <div className="now">
      {clear && (
        <div className="now-clear">
          <p className="now-clear__title">All clear.</p>
          <p className="now-clear__text">When a change waits for your yes, an agent asks you something, or a watch finds something, it lands here first.</p>
        </div>
      )}

      {decide.length > 0 && (
        <section className="now-group" aria-labelledby="now-decide">
          <h2 className="now-group__title" id="now-decide">
            Decide <span className="t-num">{decide.length}</span>
          </h2>
          {decide.map((i) => (
            <Decide key={i.key} item={i} busy={busy === i.key} act={act} online={online} />
          ))}
        </section>
      )}

      {answer.length > 0 && (
        <section className="now-group" aria-labelledby="now-answer">
          <h2 className="now-group__title" id="now-answer">
            Answer <span className="t-num">{answer.length}</span>
          </h2>
          {answer.map((i) => (
            <AnswerCard key={i.key} item={i} question={i.question!} busy={busy === i.key} act={act} online={online} />
          ))}
        </section>
      )}

      {know.length > 0 && (
        <section className="now-group" aria-labelledby="now-know">
          <h2 className="now-group__title" id="now-know">
            Know <span className="t-num">{know.length}</span>
          </h2>
          <ul className="now-know" role="list">
            {know.map((i) => (
              <li key={i.key} className={cx("now-know__item", i.state === "new" && "is-new")}>
                <span className="now-know__dot" aria-hidden />
                <div className="now-know__body">
                  <p className="now-know__title" data-user-content>
                    {i.title}
                  </p>
                  <p className="now-know__meta" title={i.source}>
                    {sourceLabel(i)} · {relativeTime(i.first_seen)}
                    {i.state === "snoozed" ? " · back from later" : ""}
                  </p>
                </div>
                <span className="now-know__actions">
                  {placeOf(i) && (
                    <Button size="sm" variant="secondary" href={hrefFor(placeOf(i)!.path)}>
                      {placeOf(i)!.label}
                    </Button>
                  )}
                  {i.link && <IconButton size="sm" variant="ghost" label="Open" icon={<ExternalLink />} href={i.link} />}
                  <Menu
                    align="end"
                    aria-label="Remind me later"
                    trigger={<IconButton size="sm" variant="ghost" label="Later" icon={<BellOff />} disabled={!online} />}
                    items={[
                      { label: "In 30 minutes", onSelect: () => void act(i.key, () => client.snooze(i.key, 30), "Snoozed") },
                      { label: "In 2 hours", onSelect: () => void act(i.key, () => client.snooze(i.key, 120), "Snoozed") },
                      { label: "Tomorrow morning", onSelect: () => void act(i.key, () => client.snooze(i.key, untilTomorrowMorning()), "Snoozed") },
                    ]}
                  />
                  <Button size="sm" variant="secondary" loading={busy === i.key} disabled={!online} onClick={() => void act(i.key, () => client.acknowledge(i.key))}>
                    Got it
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

    </div>
  );
}

/**
 * What is working right now, at the top of Today (2026-11-14: “you can't tell it is working” is
 * the complaint people make of other assistants, so this is never one step away): each run as
 * one plain row, who is running what, how long, what it has reached so far in words, and Stop,
 * the same data and the same action as On its own's Running now. The tool it is on is the row's
 * tooltip, never its words.
 */
export function WorkingNow() {
  const today = useToday();
  const { client } = useGhost();
  const { online } = useLive();
  const { busy, act } = useAct();
  const [now, setNow] = useState(() => Date.now());
  const running = today.working.length > 0;
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [running]);
  if (!running) return null;
  return (
    <section className="now-working" aria-labelledby="now-working">
      <h2 className="sr-only" id="now-working">
        Working now
      </h2>
      <ul className="now-work" role="list">
        {today.working.map((r) => {
          const who = nameOf(today.agents, r.agent);
          const started = r.started_at ?? r.scheduled_for;
          // What the run is touching (ADR 0060): its calls so far and the latest one.
          const reach = tallyFor(today.reachDay, today.calls, `run-${r.id}`);
          const touching = [...today.calls].reverse().find((c) => c.session_id === `run-${r.id}`) ?? null;
          const queued = r.status === "queued";
          const meta = [runReason(r), reach.calls > 0 ? `${count(reach.calls)} ${reach.calls === 1 ? "call" : "calls"} so far` : queued ? "waiting for its turn" : "starting", reach.apps.length > 0 ? `in ${listWords(reach.apps.map(appWords))}` : ""].filter(Boolean).join(" · ");
          return (
            <li key={r.id} className="now-work__item">
              <span className="now-work__lane" aria-hidden>
                <span className="now-work__fill" />
              </span>
              <LineGlyph slug={r.agent} name={who} size="sm" />
              <span className="now-work__text" title={touching ? `${touching.app}.${touching.tool}` : undefined}>
                <span className="now-work__line">
                  <strong>{who}</strong> {queued ? "will run" : "is working on"} <span data-user-content>{r.name}</span>
                </span>
                <span className="now-work__meta">{meta}</span>
              </span>
              <span className="now-work__clock t-num">{queued ? "queued" : `for ${took(started, new Date(now).toISOString())}`}</span>
              <Button size="sm" variant="secondary" aria-label={`Stop ${who}’s run`} loading={busy === r.id} disabled={!online} onClick={() => void act(r.id, () => client.cancelRun(r.id), "Stopped")}>
                Stop
              </Button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function listWords(xs: string[]): string {
  return xs.length <= 1 ? (xs[0] ?? "") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}

/**
 * A change waiting for the operator's yes: the one `PendingChange` card, as everywhere. Confirm
 * calls the gate's confirm by id and nothing else (the click is confirmation evidence, ADR 0013).
 * Its "More choices" makes the decision stick for the change's own tool (ADR 0070).
 */
function Decide({ item, busy, act, online }: { item: WaitingItemView; busy: boolean; act: Act; online: boolean }) {
  const { client } = useGhost();
  const change = item.change!;
  return (
    <div className="now-decide">
      <PendingChange
        summary={needLine(item)}
        rationale={change.rationale}
        operation={`${change.source}.${change.operation}`}
        target={change.target}
        expiresAt={change.expires_at}
        busy={busy}
        disabled={!online}
        onConfirm={() => void act(item.key, () => client.confirmProposal(change.id), "Done")}
        onCancel={() => void act(item.key, () => client.cancelProposal(change.id), "Left alone; nothing changed")}
        always={change.always}
        onAlways={(choice) => void act(item.key, async () => (await client.decideAlways(change.id, choice)).outcome ?? null, alwaysDone(choice, change.always))}
      />
      <p className="now-decide__meta">
        {sourceLabel(item)} · {relativeTime(item.first_seen)}
        {item.link && (
          <>
            {" · "}
            <a href={item.link} target="_blank" rel="noreferrer">
              Open
            </a>
          </>
        )}
      </p>
    </div>
  );
}

/** An agent's (or a tool's) question: its choices, the operator's own words, or Decline. */
function AnswerCard({ item, question, busy, act, online }: { item: WaitingItemView; question: QuestionView; busy: boolean; act: Act; online: boolean }) {
  const { client } = useGhost();
  const [text, setText] = useState("");
  const send = (words: string) => void act(item.key, () => client.answerQuestion(question.key, words), "Answered");
  return (
    <article className="now-card is-answer" aria-label={`${question.asked_by} asks: ${question.question}`}>
      <p className="now-card__over">
        <span className="now-card__who">
          <LineGlyph slug={question.source.startsWith("agent:") ? question.source.slice(6).split(":")[0] : "ghost"} name={question.asked_by} size="sm" />
          {question.asked_by} asks
        </span>
        {expiresIn(question) && <span>{expiresIn(question)}</span>}
      </p>
      <p className="now-card__line" data-user-content>
        {question.question}
      </p>
      {question.detail && (
        <p className="now-card__why" data-user-content>
          {question.detail}
        </p>
      )}
      <div className="now-card__actions">
        {question.choices.map((c) => (
          <Button key={c} size="sm" variant="secondary" loading={busy} disabled={!online} onClick={() => send(c)}>
            <span data-user-content>{c}</span>
          </Button>
        ))}
        {!question.choices_only && (
          <form
            className="now-card__form"
            onSubmit={(e) => {
              e.preventDefault();
              if (canSend(question, text)) send(text.trim());
            }}
          >
            <Input size="sm" value={text} placeholder={question.choices.length ? "Or in your own words" : "Your answer"} aria-label={`Your answer to ${question.asked_by}`} onChange={(e) => setText(e.target.value)} maxLength={2000} />
            <Button size="sm" type="submit" disabled={!online || !canSend(question, text)} loading={busy}>
              Send
            </Button>
          </form>
        )}
        <Button size="sm" variant="ghost" loading={busy} disabled={!online} onClick={() => void act(item.key, () => client.declineQuestion(question.key), "Declined")}>
          Decline
        </Button>
      </div>
      <p className="now-card__meta">
        {sourceLabel(item)} · {relativeTime(question.asked_at)}
      </p>
    </article>
  );
}
