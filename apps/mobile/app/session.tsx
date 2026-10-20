/**
 * The live session, built for the car: Ghost's state as one presence and one word, End and
 * Mute low in thumb reach, nothing the operator has to read. Captions are optional and off by
 * default.
 *
 * When Ghost asks before a change, the question shows large with how to answer. The phone has
 * no Confirm button: the operator says yes or no, the pipeline hears it, Jev classifies it and
 * the gate writes. When the question settles the screen holds the result for a few seconds.
 * Reconnecting keeps everything on screen, stale, with the mic paused and a spinner that says
 * the phone is trying (and, after 20 seconds, for how long).
 */
import type { OutcomeView, PendingProposal, Turn } from "@ghost/client-ts";
import { errorText, lineName, pastTense, phaseText, quoted, splitSummary } from "@ghost/ui";
import * as Haptics from "expo-haptics";
import { useKeepAwake } from "expo-keep-awake";
import { useRouter } from "expo-router";
import { Bluetooth, Captions, CaptionsOff, Check, CircleX, Headphones, Mic, MicOff, Minus, Smartphone, Volume2 } from "lucide-react-native";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Pressable, View } from "react-native";

import { useDesk } from "../src/desk/context";
import type { AudioState, Route } from "../src/desk/types";
import { setPref, update, usePhone } from "../src/phone/store";
import { radius, space, useColors } from "../src/theme";
import { LineGlyph } from "../src/ui/Brand";
import { TouchButton, TouchIcon } from "../src/ui/Button";
import { TargetText } from "../src/ui/entries";
import { Cta, Screen, TopBar, useElapsed, useLostFor } from "../src/ui/parts";
import { Text } from "../src/ui/Text";
import { VoicePresence } from "../src/ui/VoicePresence";

/** How long a settled question's result stays on screen before the session comes back. */
const RESULT_MS = 3600;
/** A question that ends without a known outcome waits this long for one before saying “Left it alone”. */
const SETTLE_GRACE_MS = 900;

interface Settled {
  p: PendingProposal;
  at: number;
  expired: boolean;
}

export default function Session() {
  const router = useRouter();
  const { desk, link, status, turns, pending, outcomes, phase, online, inSession, audio } = useDesk();
  const phone = usePhone();
  const reconnecting = link === "reconnecting" || link === "offline";
  const [ending, setEnding] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lostFor = useLostFor(reconnecting);

  // ---------------------------------------------------------------- the question's result
  const [settled, setSettled] = useState<Settled | null>(null);
  const [, setTick] = useState(0);
  const lastPending = useRef<PendingProposal | null>(null);

  useEffect(() => {
    if (pending) {
      lastPending.current = pending;
      setSettled(null);
      return;
    }
    const p = lastPending.current;
    if (!p) return;
    lastPending.current = null;
    setSettled({ p, at: Date.now(), expired: Date.parse(p.expires_at) <= Date.now() + 1000 });
  }, [pending]);

  useEffect(() => {
    if (!settled) return;
    const g = setTimeout(() => setTick((t) => t + 1), SETTLE_GRACE_MS);
    const h = setTimeout(() => setSettled(null), RESULT_MS);
    return () => {
      clearTimeout(g);
      clearTimeout(h);
    };
  }, [settled]);

  const outcome = settled ? outcomes[settled.p.id] : undefined;
  const undecided = !!settled && !outcome && !settled.expired && Date.now() - settled.at < SETTLE_GRACE_MS;

  // ---------------------------------------------------------------- phone behavior
  const muted = phone.muted;
  useEffect(() => desk.setMuted(muted), [desk, muted]);
  const listening = inSession && phase === "listening" && !muted && !reconnecting && !status?.ending;
  useEffect(() => {
    if (listening && phone.prefs.haptics) void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
  }, [listening, phone.prefs.haptics]);

  // ---------------------------------------------------------------- session facts
  const slug = status?.conversation ?? "ghost";
  const name = lineName(slug);
  const startedAt = phone.startedAt ?? turns[0]?.at ?? null;
  const elapsed = useElapsed(startedAt);
  const captions = phone.prefs.captions;

  // When the session ends, from either side, the phone goes back to Home. There is no
  // summary to read: this screen is used from a car, and Activity has everything for later.
  const wasInSession = useRef(false);
  const endingRef = useRef(false);
  useEffect(() => {
    if (inSession) {
      wasInSession.current = true;
      return;
    }
    if (!wasInSession.current || endingRef.current) return;
    wasInSession.current = false;
    update({ startedAt: null, muted: false });
    router.replace("/home");
  }, [inSession, router]);

  const end = async () => {
    setEnding(true);
    endingRef.current = true;
    update({ startedAt: null, muted: false });
    router.replace("/home");
    // The call closes at once; the desk ends the session when the phone goes.
    await desk.end().catch(() => {});
  };

  // No session: the phone never opens the mic by itself. Home's idle layout, Start in thumb reach.
  if (!inSession && !settled) {
    const start = async () => {
      setStarting(true);
      setError(null);
      try {
        await desk.start();
        update({ startedAt: new Date().toISOString(), muted: false });
      } catch (e) {
        setError(errorText(e));
      } finally {
        setStarting(false);
      }
    };
    return (
      <Screen>
        <TopBar />
        <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: space[6], paddingBottom: space[12] }}>
          <VoicePresence phase={reconnecting ? "offline" : "passive"} size={96} label="" />
          <Text size="3xl" weight="semibold" align="center" accessibilityRole="header" style={{ marginTop: space[5] }}>
            {reconnecting ? "Lost your desk for a moment" : "No session right now"}
          </Text>
          <Text size="lg" color="text2" align="center" style={{ marginTop: space[2], maxWidth: 320 }}>
            {reconnecting ? "No session was running. Start comes back as soon as the phone reaches your desk again." : "Tap Start and talk. The mic closes when you tap End."}
          </Text>
          {reconnecting && <TryingAgain lostFor={lostFor} style={{ marginTop: space[4] }} />}
        </View>
        <Cta>
          {error ? <Text size="md" color="danger" align="center" accessibilityRole="alert">{error}</Text> : null}
          <TouchButton variant="primary" size="xl" fullWidth icon={(col) => <Mic size={22} color={col} />} loading={starting} disabled={!online} onPress={start}>
            Start session
          </TouchButton>
        </Cta>
      </Screen>
    );
  }

  const question = pending ?? (undecided ? settled!.p : null);
  const showResult = !!settled && !pending && !undecided;
  // Ghost has decided to hang up: its goodbye may still be playing, then the call closes.
  // Shown over whatever phase the pipeline reports, so it never looks like it's listening.
  const hangingUp = !!status?.ending && !reconnecting;
  const presencePhase = reconnecting ? "offline" : hangingUp || muted ? "passive" : phase;
  const word = reconnecting ? "Reconnecting" : hangingUp ? "Hanging up" : muted ? "Muted" : phaseText(phase);
  const hint = hangingUp
    ? ""
    : muted
      ? "Ghost can’t hear you. Tap Unmute to talk."
      : phase === "hold"
        ? "Ghost is out of this conversation. Say “Ghost” to bring it back."
        : phase === "listening"
          ? "Go ahead."
          : phase === "starting"
            ? "Getting ready."
            : "";
  const lastTwo = captionTurns(turns);
  const showCaptions = captions && !question && !showResult && lastTwo.length > 0;

  return (
    <Screen>
      <KeepAwake on={phone.prefs.keepAwake} />
      <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: space[3], minHeight: space[16], paddingTop: space[2], paddingHorizontal: space[5] }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: space[3], flexShrink: 1 }}>
          <LineGlyph slug={slug} name={name} size="lg" />
          <View>
            <Text size="xl" weight="semibold" accessibilityRole="header" accessibilityLabel={`Session on the ${name} line`}>
              {name}
            </Text>
            <Text size="md" color="text2" num accessibilityLabel={`Elapsed ${elapsed}`}>
              {elapsed}
            </Text>
          </View>
        </View>
        <View style={{ flexDirection: "row", alignItems: "center", gap: space[1] }}>
          {audio.available.length > 1 && <RouteButton audio={audio} onPick={(r) => desk.setRoute(r)} />}
          <TouchIcon
            label={captions ? "Hide captions" : "Show captions"}
            pressed={captions}
            icon={(col) => (captions ? <Captions size={22} color={col} /> : <CaptionsOff size={22} color={col} />)}
            onPress={() => setPref("captions", !captions)}
          />
        </View>
      </View>

      <View style={{ flex: 1, justifyContent: "center", alignItems: "center", gap: space[6], paddingHorizontal: space[6], paddingBottom: question ? space[12] : space[6] }}>
        {showResult ? (
          <ResultView settled={settled!} outcome={outcome} />
        ) : question ? (
          <AskStage p={question} phase={presencePhase} stale={reconnecting} muted={muted} lostFor={lostFor} />
        ) : (
          <View style={{ flex: showCaptions ? 1 : undefined, alignItems: "center", justifyContent: "center" }}>
            <VoicePresence phase={presencePhase} size={320} label="" />
            <Text size="4xl" weight="semibold" align="center" accessibilityLiveRegion="polite" style={{ marginTop: space[8] }}>
              {word}
            </Text>
            {reconnecting ? (
              <>
                <TryingAgain lostFor={lostFor} style={{ marginTop: space[2] }} />
                {!showCaptions && (
                  <Text size="lg" color="text2" align="center" style={{ marginTop: space[1] }}>
                    The mic is paused until your desk is back.
                  </Text>
                )}
              </>
            ) : (
              <Text size="xl" color="text2" align="center" style={{ marginTop: space[2], minHeight: 24 }}>
                {hint}
              </Text>
            )}
          </View>
        )}

        {showCaptions && <CaptionBand turns={lastTwo} stale={reconnecting} />}
      </View>

      <View style={{ flexDirection: "row", gap: space[3], paddingTop: space[4], paddingHorizontal: space[5], paddingBottom: space[3] }}>
        <MuteButton muted={muted} disabled={reconnecting || hangingUp} onToggle={() => update({ muted: !muted })} />
        <View style={{ flex: 1 }}>
          <TouchButton variant="inverse" size="hero" fullWidth loading={ending} onPress={end}>
            End
          </TouchButton>
        </View>
      </View>
    </Screen>
  );
}

function KeepAwake({ on }: { on: boolean }) {
  return on ? <KeepAwakeOn /> : null;
}

function KeepAwakeOn() {
  useKeepAwake("ghost-session");
  return null;
}

/** The question, large, the issue, and how to answer by voice. */
function AskStage({ p, phase, stale, muted, lostFor }: { p: PendingProposal; phase: Parameters<typeof VoicePresence>[0]["phase"]; stale: boolean; muted: boolean; lostFor: number | null }) {
  const { text, target, conversation } = splitSummary(p);
  const how = stale
    ? "Ghost can’t hear you until your desk is back. The question is still open."
    : muted
      ? `You’re muted. Tap Unmute to say ${quoted("yes")}.`
      : `Say ${quoted("yes")} to go ahead, or ${quoted("no")} to leave it.`;
  return (
    <View style={{ width: "100%", alignItems: "center" }}>
      <VoicePresence phase={phase} size={96} label="" />
      <Text size="sm" weight="semibold" caps color="warn" style={{ marginTop: space[4] }}>
        Ghost is asking
      </Text>
      <Text size="2xl" weight="medium" color={stale ? "text2" : "text1"} align="center" numberOfLines={5} style={{ marginTop: space[2] }}>
        {text}
      </Text>
      <View style={{ marginTop: space[2] }}>
        <TargetText target={target} conversation={conversation} size="lg" />
      </View>
      {stale && <TryingAgain lostFor={lostFor} style={{ marginTop: space[4] }} />}
      <Text size="xl" color="text2" align="center" accessibilityLiveRegion="polite" style={{ marginTop: stale ? space[1] : space[6] }}>
        {how}
      </Text>
    </View>
  );
}

/** “Trying again…” with a spinner; after 20 seconds, how long ago the desk went away. */
function TryingAgain({ lostFor, style }: { lostFor: number | null; style?: object }) {
  const c = useColors();
  const long = lostFor != null && lostFor >= 20;
  const ago = lostFor == null ? "" : lostFor < 60 ? `${lostFor} s` : `${Math.floor(lostFor / 60)} min`;
  return (
    <View style={[{ flexDirection: "row", alignItems: "center", gap: space[2] }, style]}>
      <ActivityIndicator size="small" color={c.text2} />
      <Text size="xl" num>{long ? `Trying again · lost it ${ago} ago` : "Trying again…"}</Text>
    </View>
  );
}

/** What happened to the question, held for a few seconds. */
function ResultView({ settled, outcome }: { settled: Settled; outcome?: OutcomeView }) {
  const c = useColors();
  const { target, conversation } = splitSummary(settled.p);
  const done = outcome?.status === "success";
  const failed = !!outcome && !done;
  const fg = done ? c.ok : failed ? c.danger : c.text2;
  const bg = done ? c.okSoft : failed ? c.dangerSoft : c.bgActive;
  const title = done ? pastTense(settled.p.operation) : failed ? "Didn’t finish" : "Left it alone";
  const body = done ? null : failed ? (outcome.error ?? "Ghost couldn’t finish the change. It’s in Activity with what went wrong.") : settled.expired ? "No answer in time, so nothing changed." : "Nothing changed.";
  return (
    <View accessibilityLiveRegion="polite" style={{ alignItems: "center" }}>
      <View style={{ width: 96, height: 96, borderRadius: radius.full, backgroundColor: bg, alignItems: "center", justifyContent: "center" }}>
        {done ? <Check size={44} color={fg} /> : failed ? <CircleX size={44} color={fg} /> : <Minus size={44} color={fg} />}
      </View>
      <Text size="4xl" weight="semibold" align="center" style={{ marginTop: space[6] }}>{title}</Text>
      <View style={{ marginTop: space[2] }}>
        <TargetText target={target} conversation={conversation} size="xl" />
      </View>
      {body ? <Text size="lg" color="text2" align="center" style={{ marginTop: space[2], maxWidth: 320 }}>{body}</Text> : null}
    </View>
  );
}

/** The last thing the operator said and Ghost's reply, in a reserved band, never an overlay. */
function CaptionBand({ turns, stale }: { turns: Turn[]; stale: boolean }) {
  return (
    <View accessibilityLabel="Captions" style={{ width: "100%", minHeight: 112, gap: space[3] }}>
      {stale && <Text size="md" weight="medium" color="text3">Last heard before the drop · mic paused until your desk is back</Text>}
      {turns.map((t, i) =>
        t.role === "user" ? (
          <View key={`${t.at}-${i}`} style={{ gap: space[1] }}>
            <Text size="md" weight="medium" color="text3">You</Text>
            <Text size="lg" color={stale ? "text3" : "text2"} numberOfLines={2}>{t.text}</Text>
          </View>
        ) : (
          <Text key={`${t.at}-${i}`} size="xl" color="text2" numberOfLines={3}>{t.text}</Text>
        ),
      )}
    </View>
  );
}

const ROUTE_LABEL: Record<Route, string> = { speaker: "Speaker", earpiece: "Earpiece", bluetooth: "Bluetooth", headset: "Headset" };
const ROUTE_ICON = { speaker: Volume2, earpiece: Smartphone, bluetooth: Bluetooth, headset: Headphones } as const;

/**
 * Where the call's audio goes, shown only when there is a choice (a car, a headset). One tap
 * moves to the next route; the icon is the one in use, so a glance says where Ghost's voice is.
 */
function RouteButton({ audio, onPick }: { audio: AudioState; onPick: (r: Route) => void }) {
  const i = audio.available.indexOf(audio.route);
  const next = audio.available[(i + 1) % audio.available.length] ?? audio.route;
  const Icon = ROUTE_ICON[audio.route];
  return <TouchIcon label={`Audio on ${ROUTE_LABEL[audio.route]}. Switch to ${ROUTE_LABEL[next]}`} icon={(col) => <Icon size={22} color={col} />} onPress={() => onPick(next)} />;
}

/** Mute, beside End: the phone stops sending its mic, Ghost keeps the session. */
function MuteButton({ muted, disabled, onToggle }: { muted: boolean; disabled: boolean; onToggle: () => void }) {
  const c = useColors();
  const fg = disabled ? c.textDisabled : c.text1;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={muted ? "Unmute" : "Mute"}
      accessibilityState={{ selected: muted, disabled }}
      disabled={disabled}
      onPress={onToggle}
      style={({ pressed }) => ({
        width: "30%",
        height: space[20],
        borderRadius: radius.full,
        borderWidth: 1,
        borderColor: disabled ? "transparent" : muted ? c.warnBorder : c.borderStrong,
        backgroundColor: disabled ? c.bgActive : muted ? c.warnSoft : pressed ? c.bgActive : c.bgSurface,
        alignItems: "center",
        justifyContent: "center",
        gap: space[1],
      })}
    >
      {muted ? <MicOff size={24} color={fg} /> : <Mic size={24} color={fg} />}
      <Text size="md" weight="semibold" style={{ color: fg }}>{muted ? "Unmute" : "Mute"}</Text>
    </Pressable>
  );
}

function captionTurns(turns: Turn[]): Turn[] {
  return turns.filter((t) => t.role !== "system" && t.text.trim()).slice(-2);
}
