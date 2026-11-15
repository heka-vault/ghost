/**
 * The talk bar: Ghost from Today without leaving it. Ghost's presence and its state in words,
 * a field to type to it (the turn goes through the same pipeline, tools and gate as speech, and
 * the conversation opens to show it), and the voice control: Talk wakes Ghost, which is the
 * moment audio stops being local-only, so it is always a labelled button. In a session it is the
 * way into the conversation instead.
 *
 * In a live session it is the live strip (2026-11-14: the in-window room is gone, so a session
 * that doesn't float is seen right here, ADR 0059 as amended): the state in the conversation
 * screen's own words (who is speaking, what Ghost is reaching, on hold, on the phone), and right
 * above it a change waiting for the operator's yes with Confirm and Cancel, or a question asked
 * in this session with its choices. All of it from the events the window already has (ADR 0055,
 * ADR 0058); nothing new is asked of Ghost.
 */
import type { AlwaysChoice, QuestionView } from "@ghost/client-ts";
import { callsOf, useGhost, youSpeaking } from "@ghost/ui";
import { ArrowRight, ArrowUp, Mic, MicOff } from "lucide-react";
import { useState } from "react";

import { notListening, notListeningSub, typedSession } from "../../app/listening";
import { navigate } from "../../app/router";
import { nameOf, useToday } from "../../app/today";
import { Button, IconButton, LineGlyph, PendingChange, SpeakingWave, VoicePresence, type GhostPhase, alwaysDone, cx, errorText, useLive, useToast } from "../../ui";
import { stageState, stageWords } from "../session/Stage";
import { useSessionQuestions } from "../session/live";
import { liveQuestion } from "./live";

/** The session's live state, for the talk bar and for Now (which leaves out what the strip shows). */
export function useLiveSession() {
  const { status, turns, heldTurns, activity, pending } = useGhost();
  const { online, phase } = useLive();
  const { agents } = useToday();
  const inSession = !!status?.session_id;
  const onPhone = inSession && status?.session_origin === "phone";
  const who = nameOf(agents, status?.conversation);
  const apps = status?.apps ?? [];
  const calls = callsOf(activity, apps);
  const youTalking = inSession && (youSpeaking(activity) || heldTurns > 0);
  const state = stageState({ online, phase, youTalking, ending: !!status?.ending, onPhone });
  const off = notListening(status);
  const micOff = off && !onPhone ? { text: notListeningSub(off), onListen: () => undefined } : null;
  const words = stageWords(state, who, calls, micOff, typedSession(status));
  const questions = useSessionQuestions(inSession ? (turns[0]?.at ?? null) : null);
  const question = inSession ? liveQuestion(questions) : null;
  return { inSession, onPhone, who, state, words, pending: inSession ? pending : null, question };
}

export type LiveSession = ReturnType<typeof useLiveSession>;

/** `live`: `useLiveSession()`, read once by Today for this bar and for Now. */
export function TalkBar({ live }: { live: LiveSession }) {
  const { client, status } = useGhost();
  const { online, phase, loadingModels } = useLive();
  const toast = useToast();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"send" | "wake" | "confirm" | "cancel" | "answer" | null>(null);
  const { inSession, who } = live;
  const off = notListening(status);
  const presence: GhostPhase = !online ? "offline" : loadingModels ? "starting" : phase;
  const words = !online
    ? "Ghost isn’t answering"
    : inSession
      ? live.words.title
      : off
        ? "Not listening · typing still works"
        : loadingModels
          ? "Loading its voice"
          : "Say “Ghost”, or type";
  // The listening hints point at the conversation screen's composer (“type below”); here the field is beside them.
  const hint = online && inSession && live.state !== "listening" ? live.words.hint : undefined;

  const send = async () => {
    const t = text.trim();
    if (!t) return;
    setBusy("send");
    try {
      await client.say(t);
      setText("");
      navigate("/session");
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t send that", description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };
  const wake = async () => {
    setBusy("wake");
    try {
      await client.wake();
      navigate("/session");
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t wake Ghost", description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };
  // The session's own confirm and cancel, as on the conversation screen: the click is
  // confirmation evidence from this desk (ADR 0013, `classifier: ui`).
  const settle = async (action: "confirm" | "cancel") => {
    setBusy(action);
    try {
      await (action === "confirm" ? client.confirm() : client.cancel());
    } catch (e) {
      toast({ tone: "danger", title: action === "confirm" ? "Couldn’t confirm" : "Couldn’t cancel", description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };
  // ADR 0070: the same decision made to stick, from the strip's card. The daemon takes the rule
  // from the waiting change, then confirms or cancels it as the plain buttons do.
  const settleAlways = async (choice: AlwaysChoice) => {
    const p = live.pending;
    if (!p) return;
    setBusy(choice === "allow" ? "confirm" : "cancel");
    try {
      await client.decideAlways(p.id, choice);
      toast(alwaysDone(choice, p.always?.scope ?? "this tool"));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t save that choice", description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };
  const answer = async (q: QuestionView, choice: string) => {
    setBusy("answer");
    try {
      await client.answerQuestion(q.key, choice);
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t answer", description: errorText(e) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className={cx("talk-dock", inSession && "is-live")}>
      {live.pending ? (
        <PendingChange
          className="talk-live__card"
          compact
          summary={live.pending.summary}
          rationale={live.pending.rationale}
          operation={`${live.pending.source}.${live.pending.operation}`}
          expiresAt={live.pending.expires_at}
          busy={busy === "confirm" ? "confirm" : busy === "cancel" ? "cancel" : false}
          disabled={!online}
          onConfirm={() => void settle("confirm")}
          onCancel={() => void settle("cancel")}
          always={live.pending.always}
          onAlways={(c) => void settleAlways(c)}
        />
      ) : live.question ? (
        <LiveQuestion q={live.question} busy={busy === "answer"} online={online} onAnswer={(c) => void answer(live.question!, c)} />
      ) : null}
      <form
        className={cx("talk", inSession && "is-live", inSession && `is-${live.state}`)}
        aria-label="Talk to Ghost"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <span className="talk__presence">{phase === "speaking" && online ? <SpeakingWave size={48} /> : <VoicePresence phase={presence} size={48} label="" />}</span>
        <span className="talk__state" role="status">
          {off && <MicOff aria-hidden className="talk__off" />}
          <span className="talk__title">{words}</span>
          {hint && <span className="talk__hint">{hint}</span>}
        </span>
        <input
          className="talk__input"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={inSession ? `Type to ${who}…` : "Type to Ghost…"}
          aria-label={inSession ? `Type to ${who}` : "Type to Ghost"}
          disabled={!online}
          maxLength={4000}
        />
        {text.trim() ? (
          <IconButton type="submit" variant="primary" label="Send" icon={<ArrowUp />} loading={busy === "send"} disabled={!online} />
        ) : inSession ? (
          <Button variant="secondary" trailingIcon={<ArrowRight />} onClick={() => navigate("/session")}>
            Open the conversation
          </Button>
        ) : (
          <Button variant="secondary" leadingIcon={<Mic />} loading={busy === "wake"} disabled={!online || loadingModels} onClick={() => void wake()} title="Wake Ghost to talk">
            Talk
          </Button>
        )}
      </form>
    </div>
  );
}

/** A question asked in this session (ADR 0051): the asker's words, its choices, or the way to answer in words. */
function LiveQuestion({ q, busy, online, onAnswer }: { q: QuestionView; busy: boolean; online: boolean; onAnswer: (choice: string) => void }) {
  const slug = q.source.startsWith("agent:") ? q.source.slice(6).split(":")[0] : "ghost";
  return (
    <article className="talk-live__card talk-ask" aria-label={`${q.asked_by} asks: ${q.question}`}>
      <p className="talk-ask__line">
        <LineGlyph slug={slug} name={q.asked_by} size="sm" />
        <span className="talk-ask__who">{q.asked_by} asks</span>
        <span className="talk-ask__q" data-user-content>
          {q.question}
        </span>
      </p>
      <div className="talk-ask__actions">
        {q.choices.map((c) => (
          <Button key={c} size="sm" variant="secondary" loading={busy} disabled={!online} onClick={() => onAnswer(c)}>
            <span data-user-content>{c}</span>
          </Button>
        ))}
        {!q.choices_only && (
          <Button size="sm" variant="ghost" trailingIcon={<ArrowRight />} onClick={() => navigate("/session")}>
            Answer in words
          </Button>
        )}
      </div>
    </article>
  );
}
