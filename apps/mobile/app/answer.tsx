/**
 * A ring answered (ADR 0051, issue #22). The ring screen or the call notification opens
 * `ghost://answer?ring=…&agent=…&item=…`; the native side has already stopped the ringing and
 * told the desk "answered". Here the app waits for that answer, then opens an ordinary session
 * through the usual offer, in the agent that called and naming the item, so Ghost opens with
 * it. A ring that was over by then says so, and calling back is one tap. Nothing here opens
 * the microphone without the operator having answered.
 */
import { errorText, lineName } from "@ghost/ui";
import { useLocalSearchParams, useRouter } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { View } from "react-native";

import { useDesk } from "../src/desk/context";
import type { Desk } from "../src/desk/types";
import { parseRingAnswer } from "../src/desk/waiting";
import { update } from "../src/phone/store";
import { space } from "../src/theme";
import { TouchButton } from "../src/ui/Button";
import { LineGlyph } from "../src/ui/Brand";
import { Cta, Screen } from "../src/ui/parts";
import { Text } from "../src/ui/Text";
import { VoicePresence } from "../src/ui/VoicePresence";

/** How long the app waits for the desk to answer `/hello` after a cold start. */
const ONLINE_MS = 15_000;

type Stage = { kind: "joining" } | { kind: "over" } | { kind: "failed"; message: string };

export default function Answer() {
  const router = useRouter();
  const params = useLocalSearchParams();
  const ring = parseRingAnswer(params);
  const { desk } = useDesk();
  const [stage, setStage] = useState<Stage>({ kind: "joining" });
  const started = useRef(false);
  const name = lineName(ring?.agent ?? "ghost");

  const open = async (afterRing: boolean) => {
    if (!ring) return;
    setStage({ kind: "joining" });
    try {
      if (afterRing && !(await desk.answerRing(ring.ringId))) {
        setStage({ kind: "over" });
        return;
      }
      await online(desk);
      await desk.start(ring.agent, ring.item ?? undefined);
      update({ startedAt: new Date().toISOString(), muted: false });
      router.replace("/session");
    } catch (e) {
      setStage({ kind: "failed", message: errorText(e) });
    }
  };

  useEffect(() => {
    if (!ring) {
      router.replace("/home");
      return;
    }
    if (started.current) return;
    started.current = true;
    void open(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per answered ring
  }, [ring?.ringId]);

  const title = stage.kind === "joining" ? `Connecting to ${name}` : stage.kind === "over" ? `${name} stopped calling` : "The call didn’t open";
  const body =
    stage.kind === "joining"
      ? "Opening the mic on this phone."
      : stage.kind === "over"
        ? "It rang for a while and moved on. It’s still on your list; call back to talk about it now."
        : stage.message;

  return (
    <Screen>
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: space[6], gap: space[3] }}>
        {stage.kind === "joining" ? <VoicePresence phase="starting" size={160} label="" /> : <LineGlyph slug={ring?.agent ?? "ghost"} name={name} size="lg" />}
        <Text size="3xl" weight="semibold" align="center" accessibilityRole="header" accessibilityLiveRegion="polite">
          {title}
        </Text>
        <Text size="lg" color="text2" align="center" style={{ maxWidth: 320 }}>
          {body}
        </Text>
      </View>
      {stage.kind !== "joining" ? (
        <Cta>
          <TouchButton variant="primary" size="xl" fullWidth onPress={() => void open(false)}>
            {`Call ${name} back`}
          </TouchButton>
          <TouchButton variant="ghost" size="lg" fullWidth onPress={() => router.replace("/home")}>
            Not now
          </TouchButton>
        </Cta>
      ) : null}
    </Screen>
  );
}

/** Resolve once the desk answers, which takes a moment when the app was opened cold by the ring. */
function online(desk: Desk): Promise<void> {
  if (desk.getState().link === "online") return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      unsubscribe();
      reject(new Error("Your desk isn’t reachable right now."));
    }, ONLINE_MS);
    const unsubscribe = desk.subscribe(() => {
      if (desk.getState().link !== "online") return;
      clearTimeout(timer);
      unsubscribe();
      resolve();
    });
  });
}
