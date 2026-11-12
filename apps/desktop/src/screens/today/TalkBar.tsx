/**
 * The talk bar: Ghost from Today without leaving it. Ghost's presence and its state in words,
 * a field to type to it (the turn goes through the same pipeline, tools and gate as speech, and
 * the conversation opens to show it), and the voice control: Talk wakes Ghost, which is the
 * moment audio stops being local-only, so it is always a labelled button. In a session it is the
 * way into the conversation instead.
 */
import { useGhost } from "@ghost/ui";
import { ArrowRight, ArrowUp, Mic, MicOff } from "lucide-react";
import { useState } from "react";

import { notListening } from "../../app/listening";
import { navigate } from "../../app/router";
import { nameOf, useToday } from "../../app/today";
import { Button, IconButton, SpeakingWave, VoicePresence, type GhostPhase, cx, errorText, formatShortcut, useLive, useToast } from "../../ui";

export function TalkBar() {
  const { client, status } = useGhost();
  const { online, phase, loadingModels } = useLive();
  const { agents } = useToday();
  const toast = useToast();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"send" | "wake" | null>(null);
  const inSession = !!status?.session_id;
  const off = notListening(status);
  const presence: GhostPhase = !online ? "offline" : loadingModels ? "starting" : phase;
  const who = nameOf(agents, status?.conversation);
  const words = !online
    ? "Ghost isn’t answering"
    : inSession
      ? `In a conversation with ${who}`
      : off
        ? "Not listening · typing still works"
        : loadingModels
          ? "Loading its voice"
          : "Say “Ghost”, or type";

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

  return (
    <form
      className={cx("talk", inSession && "is-live")}
      aria-label="Talk to Ghost"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <span className="talk__presence">{phase === "speaking" && online ? <SpeakingWave size={48} /> : <VoicePresence phase={presence} size={48} label="" />}</span>
      <span className="talk__state">
        {off && <MicOff aria-hidden className="talk__off" />}
        {words}
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
        <Button variant="secondary" leadingIcon={<Mic />} loading={busy === "wake"} disabled={!online || loadingModels} onClick={() => void wake()} title={`Wake Ghost to talk (${formatShortcut("mod+.")} for the room)`}>
          Talk
        </Button>
      )}
    </form>
  );
}
