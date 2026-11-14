/**
 * The brand's workbench, as the desktop's `#/mascot`: the 320 presence in every phase, the marks
 * and every agent's glyph on one page, so a change to the mark can be looked at (and screenshot
 * by `scripts/shots.mjs`) without walking through a session. Only on the mock desk (the web
 * build and `EXPO_PUBLIC_GHOST_DESK=mock`); a real build redirects home.
 *
 * `?only=<phase>` shows one phase's presence alone, for measuring a mood's frame cost.
 */
import type { GhostPhase } from "@ghost/ui";
import { Redirect, useLocalSearchParams } from "expo-router";
import { ScrollView, View } from "react-native";

import { MOCK_DESK } from "../src/phone/pairing";
import { space, useColors } from "../src/theme";
import { AgentMark, GhostGlyph, GhostMark, LineGlyph } from "../src/ui/Brand";
import { Screen } from "../src/ui/parts";
import { Text } from "../src/ui/Text";
import { VoicePresence } from "../src/ui/VoicePresence";

const PHASES: { phase: GhostPhase; float?: boolean; name: string }[] = [
  { phase: "passive", float: true, name: "Home, ready (the Start control)" },
  { phase: "passive", name: "Passive" },
  { phase: "listening", name: "Listening" },
  { phase: "thinking", name: "Thinking" },
  { phase: "speaking", name: "Speaking" },
  { phase: "hold", name: "Hold" },
  { phase: "starting", name: "Starting" },
  { phase: "offline", name: "Offline" },
];

const AGENTS: { slug: string; look: string | null; name: string }[] = [
  { slug: "ghost", look: null, name: "Ghost" },
  { slug: "q3-audit", look: null, name: "No look (Wisp)" },
  { slug: "heka-workshop", look: "star:5", name: "star:5" },
  { slug: "acme-research", look: "moth:7", name: "moth:7" },
  { slug: "accounting", look: "lantern:4", name: "lantern:4" },
];

export default function Brand() {
  const c = useColors();
  const { only } = useLocalSearchParams<{ only?: string }>();
  if (!MOCK_DESK) return <Redirect href="/home" />;
  const one = PHASES.find((p) => p.phase === only && !p.float);
  if (one) {
    return (
      <Screen>
        <View style={{ flex: 1, alignItems: "center", justifyContent: "center" }}>
          <VoicePresence phase={one.phase} size={320} label="" />
        </View>
      </Screen>
    );
  }
  return (
    <Screen>
      <ScrollView contentContainerStyle={{ padding: space[5], gap: space[6] }}>
        <Text size="3xl" weight="semibold">The mark</Text>
        <View style={{ flexDirection: "row", gap: space[4], alignItems: "flex-end", flexWrap: "wrap" }}>
          {[24, 32, 40, 64, 96].map((s) => (
            <View key={s} style={{ alignItems: "center", gap: space[1] }}>
              <GhostMark size={s} />
              <Text size="xs" color="text2">{s}</Text>
            </View>
          ))}
        </View>
        <Text size="xl" weight="semibold">The glyph</Text>
        <View style={{ flexDirection: "row", gap: space[4], alignItems: "flex-end", flexWrap: "wrap" }}>
          {[16, 20, 24, 32, 44, 56].map((s) => (
            <View key={s} style={{ alignItems: "center", gap: space[1] }}>
              <GhostGlyph size={s} color={c.text1} />
              <Text size="xs" color="text2">{s}</Text>
            </View>
          ))}
        </View>
        <Text size="xl" weight="semibold">Agents</Text>
        <View style={{ flexDirection: "row", gap: space[4], flexWrap: "wrap" }}>
          {AGENTS.map((a) => (
            <View key={a.slug} style={{ alignItems: "center", gap: space[1], width: 88 }}>
              <AgentMark slug={a.slug} look={a.look} size={40} />
              <LineGlyph slug={a.slug} name={a.name} size="md" />
              <Text size="xs" color="text2" align="center">{a.name}</Text>
            </View>
          ))}
        </View>
        <Text size="xl" weight="semibold">The presence</Text>
        <View style={{ flexDirection: "row", flexWrap: "wrap", gap: space[2] }}>
          {PHASES.map((p) => (
            <View key={p.name} style={{ alignItems: "center", gap: space[1] }}>
              <VoicePresence phase={p.phase} size={320} label="" float={p.float} />
              <Text size="md" color="text2">{p.name}</Text>
            </View>
          ))}
        </View>
        <Text size="xl" weight="semibold">Small presences</Text>
        <View style={{ flexDirection: "row", flexWrap: "wrap", gap: space[3] }}>
          {PHASES.filter((p) => !p.float).map((p) => (
            <View key={p.name} style={{ alignItems: "center", gap: space[1] }}>
              <VoicePresence phase={p.phase} size={96} label="" />
              <Text size="xs" color="text2">{p.name}</Text>
            </View>
          ))}
        </View>
      </ScrollView>
    </Screen>
  );
}
