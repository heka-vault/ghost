/**
 * Home: the ghost is the Start control: it floats while Ghost is ready, and a tap opens the
 * mic into a session with Ghost. Under it, the agents (ADR 0014, ADR 0035) as a dial pad:
 * three big keys to a row, each a ghost in the agent's own colour over its name, so an agent
 * is known by colour and a press, not a scroll, calls it directly (the session opens there,
 * greeting as that agent). Nothing listens here; the mic opens only after a tap.
 * Activity has the history. While Ghost is asking, the question is the hero.
 */
import { errorText, lineHue, lineName, splitSummary } from "@ghost/ui";
import { useRouter } from "expo-router";
import { AudioLines } from "lucide-react-native";
import { useEffect, useState } from "react";
import { Pressable, ScrollView, useWindowDimensions, View } from "react-native";

import { useDesk } from "../../src/desk/context";
import { update, usePhone } from "../../src/phone/store";
import { opacityDisabled, radius, space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { GhostGlyph } from "../../src/ui/Brand";
import { TargetText } from "../../src/ui/entries";
import { Cta, Divider, LinkDot, linkLabel, List, Row, Screen, Skeleton, viaLabel } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";
import { VoicePresence } from "../../src/ui/VoicePresence";

/** Decided once per app launch, so turning the preference on later doesn't start a session on the spot. */
let launchChecked = false;
let launchWantsStart = false;

export default function Home() {
  const router = useRouter();
  const { desk, link, status, pending, phase, online, loadingModels, inSession, deskBusy, via, conversations, waiting } = useDesk();
  const phone = usePhone();
  const { pairing } = phone;
  const { tone, title } = linkLabel(link);
  const [starting, setStarting] = useState(false);
  const [wave, setWave] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const line = status?.conversation && status.conversation !== "ghost" ? `${lineName(status.conversation)} agent` : "Ghost";
  // The dial pad: Ghost, then every working agent by last use, one key each (ADR 0035: no grouping).
  const keys = conversations
    .filter((c) => !c.retired_at)
    .sort((a, b) => (a.is_ghost ? -1 : b.is_ghost ? 1 : (b.last_active_at ?? "").localeCompare(a.last_active_at ?? "")));
  const colors = useColors();
  // Three keys to a row, like a dial pad: big enough that a thumb never hits the wrong line.
  const { width } = useWindowDimensions();
  const keyGap = space[3];
  const keySize = Math.floor((width - 2 * space[5] - 2 * keyGap) / 3);
  const workstation = pairing?.workstation ?? "your desk";
  // The waiting list, without what is snoozed (ADR 0029).
  const shownWaiting = (waiting ?? []).filter((i) => i.state !== "snoozed");
  const [acting, setActing] = useState<string | null>(null);
  const act = async (key: string, run: () => Promise<void>) => {
    setActing(key);
    try {
      await run();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setActing(null);
    }
  };
  const ready = online && !loadingModels && !deskBusy && !inSession;

  const start = async (conversation?: string) => {
    if (!ready || starting) return;
    // Wisp waves while the desk opens the session.
    setWave((w) => w + 1);
    setStarting(true);
    setError(null);
    try {
      await desk.start(conversation);
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
    hero = { presence: phase, title: "You’re in a session", body: `With ${line}. The mic is open on this phone.` };
  } else if (deskBusy) {
    hero = { presence: phase, title: "Ghost is busy at your desk", body: `A session is running with ${line} at ${workstation}. Start here once it ends.` };
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

      <ScrollView contentContainerStyle={{ flexGrow: 1 }} keyboardShouldPersistTaps="handled">
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: space[6], paddingVertical: space[8] }}>
        {link === "connecting" ? (
          <View style={{ alignItems: "center", gap: space[4] }}>
            <Skeleton width={320} height={320} round />
            <Skeleton width={220} height={28} />
          </View>
        ) : (
          <>
            {/* The ghost is the button: the 320 hero, the screen's main control. It floats only while a tap would start. */}
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Start session"
              accessibilityHint="Opens the mic on this phone"
              accessibilityState={{ disabled: !ready, busy: starting }}
              disabled={!ready}
              onPress={() => start()}
              hitSlop={24}
              style={({ pressed }) => ({ opacity: ready || inSession ? (pressed ? 0.82 : 1) : opacityDisabled, transform: [{ scale: pressed && ready ? 0.96 : 1 }] })}
            >
              <VoicePresence phase={hero.presence} size={320} label={hero.question ? undefined : ""} float={ready && !starting} wave={wave} />
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

      {/* What needs you (ADR 0029): read while the app is open; there is no push. A change
          waiting for a yes is answered by voice, so a tap on one calls Ghost. */}
      {shownWaiting.length > 0 && !inSession && link !== "connecting" ? (
        <View style={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
          <Text size="md" weight="semibold" color="text2" style={{ marginBottom: space[3], marginLeft: space[1] }}>
            What needs you
          </Text>
          <List>
            {shownWaiting.map((item, i) => (
              <View key={item.key}>
                {i > 0 && <Divider />}
                <Row
                  top
                  title={<Text size="lg" numberOfLines={2}>{item.title}</Text>}
                  support={
                    <Text size="md" color="text3" numberOfLines={1}>
                      {waitingSource(item)}
                      {item.state === "acknowledged" ? " · heard" : ""}
                    </Text>
                  }
                  accessibilityLabel={item.source === "gate" ? `${item.title}. Call Ghost to answer it.` : item.title}
                  onPress={item.source === "gate" && ready ? () => void start() : undefined}
                  trailing={
                    item.source === "gate" ? null : (
                      <View style={{ gap: space[2] }}>
                        {item.state !== "acknowledged" ? (
                          <TouchButton size="md" loading={acting === item.key} onPress={() => void act(item.key, () => desk.acknowledge(item.key))}>
                            Got it
                          </TouchButton>
                        ) : null}
                        <TouchButton size="md" variant="ghost" onPress={() => void act(item.key, () => desk.snooze(item.key, 120))}>
                          Later
                        </TouchButton>
                      </View>
                    )
                  }
                />
              </View>
            ))}
          </List>
        </View>
      ) : null}

      {/* The dial pad: Ghost and a key per agent. A press calls that agent. */}
      {keys.length > 1 && !inSession && link !== "connecting" ? (
        <View style={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
          <Text size="md" weight="semibold" color="text2" style={{ marginBottom: space[3], marginLeft: space[1] }}>
            Agents
          </Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: keyGap, opacity: ready ? 1 : opacityDisabled }}>
            {keys.map((c) => {
              const hue = colors.hue[lineHue(c.slug)];
              const name = lineName(c.slug, c.name);
              return (
                <Pressable
                  key={c.slug}
                  accessibilityRole="button"
                  accessibilityLabel={`Call ${name}`}
                  accessibilityHint="Opens the mic on this phone, with that agent"
                  accessibilityState={{ disabled: !ready }}
                  disabled={!ready}
                  onPress={() => void start(c.slug)}
                  style={({ pressed }) => ({
                    width: keySize,
                    height: keySize,
                    borderRadius: radius.lg,
                    borderWidth: 1,
                    borderColor: pressed ? hue.fg : colors.border,
                    backgroundColor: pressed ? hue.bg : colors.bgSurface,
                    alignItems: "center",
                    justifyContent: "center",
                    gap: space[2],
                    paddingHorizontal: space[2],
                    transform: [{ scale: pressed ? 0.97 : 1 }],
                  })}
                >
                  <GhostGlyph size={Math.round(keySize * 0.4)} color={hue.fg} />
                  <Text size="md" weight="semibold" align="center" numberOfLines={2} style={{ color: colors.text1 }}>
                    {name}
                  </Text>
                </Pressable>
              );
            })}
          </View>
        </View>
      ) : null}
      </ScrollView>

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

/** Where an item came from, in plain words. */
function waitingSource(item: { source: string; delivered_via?: string | null }): string {
  const [kind, rest] = item.source.includes(":") ? [item.source.slice(0, item.source.indexOf(":")), item.source.slice(item.source.indexOf(":") + 1)] : [item.source, ""];
  if (kind === "watch") return `Watch ${rest}`;
  if (kind === "hook") return `Webhook ${rest}`;
  if (kind === "gate") return "Waiting for your yes · tap to call Ghost";
  return "Ghost";
}

