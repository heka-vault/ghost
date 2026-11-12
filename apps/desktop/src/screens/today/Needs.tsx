/**
 * Now: the head of Today's timeline. What needs the operator, by what they do about it (Decide,
 * Answer, Know), each with its one action in reach; then the agents at work right now, with
 * live clocks and a way to stop a run. Every action the retired What needs you screen had is
 * here: Confirm and Cancel (the click is confirmation evidence, ADR 0013), an answer in the
 * operator's own words or a choice, Decline (ADR 0051), Got it, Later and Open (ADR 0029).
 *
 * Titles, questions and choices are other systems' and agents' words: shown as text only.
 */
import type { QuestionView, WaitingItemView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { BellOff, ExternalLink } from "lucide-react";
import { useEffect, useState } from "react";

import { nameOf, useToday } from "../../app/today";
import { Button, IconButton, Input, LineGlyph, Menu, PendingChange, cx, errorText, relativeTime, useLive, useToast } from "../../ui";
import { clock, needLine } from "./model";
import { actionFailure, canSend, expiresIn, sourceLabel, untilTomorrowMorning } from "./needs";

type Act = (key: string, run: () => Promise<unknown>, done?: string) => Promise<void>;

function useAct(): { busy: string | null; act: Act } {
  const toast = useToast();
  const { reload } = useToday();
  const [busy, setBusy] = useState<string | null>(null);
  const act: Act = async (key, run, done) => {
    setBusy(key);
    try {
      const failed = actionFailure(await run());
      if (failed) toast({ title: failed.title, description: failed.text || undefined, tone: failed.tone });
      else if (done) toast({ title: done });
      reload();
    } catch (e) {
      toast({ title: "That didn’t work", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  return { busy, act };
}


export function Now() {
  const today = useToday();
  const { client } = useGhost();
  const { online } = useLive();
  const { busy, act } = useAct();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  const { decide, answer, know } = today.needs;
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
                  <p className="now-know__meta">
                    {sourceLabel(i)} · {relativeTime(i.first_seen)}
                    {i.state === "delivered" && i.delivered_via ? ` · sent by ${i.delivered_via}, not heard yet` : ""}
                    {i.state === "snoozed" ? " · back from later" : ""}
                  </p>
                </div>
                <span className="now-know__actions">
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

      {today.working.length > 0 && (
        <section className="now-group" aria-labelledby="now-working">
          <h2 className="now-group__title" id="now-working">
            Working now <span className="t-num">{today.working.length}</span>
          </h2>
          <ul className="now-work" role="list">
            {today.working.map((r) => {
              const who = nameOf(today.agents, r.agent);
              const started = r.started_at ?? r.scheduled_for;
              return (
                <li key={r.id} className="now-work__item">
                  <span className="now-work__lane" aria-hidden>
                    <span className="now-work__fill" />
                  </span>
                  <LineGlyph slug={r.agent} name={who} size="sm" />
                  <span className="now-work__text">
                    <span className="now-work__line">
                      <strong>{who}</strong> is running <span data-user-content>{r.name}</span>
                    </span>
                    <span className="now-work__meta">
                      {r.source.startsWith("watch:") ? `for the ${r.source.slice(6)} watch` : r.source === "run_now" ? "when asked" : "on its schedule"} · {r.model_calls} model · {r.tool_calls} tool {r.tool_calls === 1 ? "call" : "calls"}
                    </span>
                  </span>
                  <span className="now-work__clock t-num" aria-label={`running for ${clock(started, now)}`}>
                    {r.status === "queued" ? "queued" : clock(started, now)}
                  </span>
                  <Button size="sm" variant="ghost" aria-label={`Stop ${who}’s run`} loading={busy === r.id} disabled={!online} onClick={() => void act(r.id, () => client.cancelRun(r.id), "Stopped")}>
                    Stop
                  </Button>
                </li>
              );
            })}
          </ul>
        </section>
      )}
    </div>
  );
}

/**
 * A change waiting for the operator's yes: the one `PendingChange` card, as everywhere. Confirm
 * calls the gate's confirm by id and nothing else (the click is confirmation evidence, ADR 0013).
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
        expiresAt={change.expires_at}
        busy={busy}
        disabled={!online}
        onConfirm={() => void act(item.key, () => client.confirmProposal(change.id), "Done")}
        onCancel={() => void act(item.key, () => client.cancelProposal(change.id), "Canceled; nothing changed")}
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
        <span>{expiresIn(question)}</span>
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
