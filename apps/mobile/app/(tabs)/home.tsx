/**
 * Home: one thing to do. The ghost is the Start control: it floats while Ghost is ready,
 * and a tap opens the mic into a session. Nothing listens here; the mic opens only after
 * that tap. There is no list to read: this screen is used from a car, and Activity has
 * everything for later. While Ghost is asking, the question is the hero.
 */
import { errorText, lineName, splitSummary } from "@ghost/ui";
import { useRouter } from "expo-router";
import { AudioLines } from "lucide-react-native";
import { useEffect, useState } from "react";
import { Pressable, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { update, usePhone } from "../../src/phone/store";
import { opacityDisabled, space } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { GhostMark } from "../../src/ui/Brand";
import { TargetText } from "../../src/ui/entries";
import { Cta, LinkDot, linkLabel, Screen, Skeleton, viaLabel } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";
import { VoicePresence } from "../../src/ui/VoicePresence";

/** Decided once per app launch, so turning the preference on later doesn't start a session on the spot. */
let launchChecked = false;
let launchWantsStart = false;

export default function Home() {
  const router = useRouter();
  const { desk, link, status, pending, phase, online, loadingModels, inSession, deskBusy, via } = useDesk();
  const phone = usePhone();
  const { pairing } = phone;
  const { tone, title } = linkLabel(link);
  const [starting, setStarting] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const line = status?.conversation && status.conversation !== "ghost" ? `${lineName(status.conversation)} line` : "main line";
  const workstation = pairing?.workstation ?? "your desk";
  const ready = online && !loadingModels && !deskBusy && !inSession;

  const start = async () => {
    if (!ready || starting) return;
    setStarting(true);
    setError(null);
    try {
      await desk.start();
      update({ startedAt: new Date().toISOString(), muted: false });
      router.push("/session");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(false);
    }
  };

  const retry = async () => {
    setRetrying(true);
    setError(null);
    try {
      await desk.retry();
    } catch {
      setError("Still can’t reach it. The phone keeps trying on its own.");
    } finally {
      setRetrying(false);
    }
  };

  // Hands-free: opened by voice with the preference on, the first Home that can start, does.
  useEffect(() => {
    if (!phone.hydrated) return;
    if (!launchChecked) {
      launchChecked = true;
      launchWantsStart = phone.prefs.autoStart;
    }
    if (launchWantsStart && ready && !starting) {
      launchWantsStart = false;
      void start();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- start is stable enough for a one-shot
  }, [phone.hydrated, phone.prefs.autoStart, ready, starting]);

  let hero: { presence: Parameters<typeof VoicePresence>[0]["phase"]; title: string; body: string; question?: boolean };
  if (link === "offline") {
    hero = { presence: "offline", title: "Can’t reach your desk", body: `Ghost isn’t answering on ${workstation}. Check that it’s running and that this phone is on the same Wi-Fi.` };
  } else if (inSession && pending) {
    hero = { presence: phase, title: "Ghost is waiting on you", body: splitSummary(pending).text, question: true };
  } else if (inSession) {
    hero = { presence: phase, title: "You’re in a session", body: `On the ${line}. The mic is open on this phone.` };
  } else if (deskBusy) {
    hero = { presence: phase, title: "Ghost is busy at your desk", body: `A session is running on the ${line} at ${workstation}. Start here once it ends.` };
  } else if (link === "reconnecting") {
    hero = { presence: "offline", title: "Lost your desk for a moment", body: "Start comes back as soon as the phone reaches it again." };
  } else if (loadingModels) {
    hero = { presence: "starting", title: "Getting ready", body: "Ghost is loading its voice models. Usually under a minute." };
  } else if (starting) {
    hero = { presence: "starting", title: "Opening the mic", body: "" };
  } else {
    hero = { presence: "passive", title: "Tap the ghost and talk", body: "The mic stays off until you do." };
  }

  return (
    <Screen bottom={false}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: space[3], paddingTop: space[3], paddingHorizontal: space[5], paddingBottom: space[2] }}>
        <GhostMark size={32} />
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text size="xl" weight="semibold" accessibilityRole="header">Ghost</Text>
          <View accessibilityRole="text" accessibilityLiveRegion="polite" style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
            <LinkDot tone={tone} />
            <Text size="md" color="text2" numberOfLines={1} style={{ flexShrink: 1 }}>
              {title}
              {link === "online" && pairing ? <Text size="md" color="text3">{` · ${pairing.workstation}${via ? ` · ${viaLabel(via)}` : ""}`}</Text> : null}
            </Text>
          </View>
        </View>
      </View>

      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: space[6], paddingBottom: space[10] }}>
        {link === "connecting" ? (
          <View style={{ alignItems: "center", gap: space[4] }}>
            <Skeleton width={160} height={160} round />
            <Skeleton width={220} height={28} />
          </View>
        ) : (
          <>
            {/* The ghost is the button. 160 is the hero size; the glyph floats only while a tap would start. */}
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Start session"
              accessibilityHint="Opens the mic on this phone"
              accessibilityState={{ disabled: !ready, busy: starting }}
              disabled={!ready}
              onPress={start}
              hitSlop={24}
              style={({ pressed }) => ({ opacity: ready || inSession ? (pressed ? 0.82 : 1) : opacityDisabled, transform: [{ scale: pressed && ready ? 0.96 : 1 }] })}
            >
              <VoicePresence phase={hero.presence} size={160} label={hero.question ? undefined : ""} float={ready && !starting} />
            </Pressable>
            <Text size="3xl" weight="semibold" align="center" style={{ marginTop: space[6] }}>
              {hero.title}
            </Text>
            {hero.body ? (
              <Text size="lg" color={hero.question ? "text1" : "text2"} align="center" numberOfLines={hero.question ? 3 : undefined} style={{ marginTop: space[2], maxWidth: 320 }}>
                {hero.body}
              </Text>
            ) : null}
            {hero.question && pending ? (
              <View style={{ marginTop: space[2] }}>
                <TargetText {...splitSummary(pending)} size="lg" />
              </View>
            ) : null}
          </>
        )}
      </View>

      <Cta>
        {error ? (
          <Text size="md" color={link === "offline" ? "text2" : "danger"} align="center" accessibilityRole="alert">
            {error}
          </Text>
        ) : null}
        {link === "offline" ? (
          <TouchButton size="xl" fullWidth loading={retrying} onPress={retry}>
            Try again
          </TouchButton>
        ) : inSession ? (
          <TouchButton variant="primary" size="xl" fullWidth icon={(col) => <AudioLines size={22} color={col} />} onPress={() => router.push("/session")}>
            Back to the session
          </TouchButton>
        ) : null}
      </Cta>
    </Screen>
  );
}
