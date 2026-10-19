/**
 * After End: how long, which line, and what changed. Done is the single primary action, low
 * on the screen. The transcript is there but folded. To change something back, ask Ghost in
 * the next session; it makes the opposite change like any other.
 */
import { formatTime } from "@ghost/ui";
import { useRouter } from "expo-router";
import { ScrollView, View } from "react-native";

import { useDesk } from "../src/desk/context";
import { usePhone } from "../src/phone/store";
import { space } from "../src/theme";
import { LineGlyph } from "../src/ui/Brand";
import { TouchButton } from "../src/ui/Button";
import { EntryRow } from "../src/ui/entries";
import { Cta, Disclosure, Divider, durationWords, List, Notice, Screen, SectionHead } from "../src/ui/parts";
import { Text } from "../src/ui/Text";

export default function Summary() {
  const router = useRouter();
  const { audit } = useDesk();
  const { ended: s } = usePhone();

  if (!s) {
    return (
      <Screen>
        <View style={{ flex: 1, alignItems: "center", justifyContent: "center", padding: space[6] }}>
          <Text size="2xl" weight="semibold" align="center">No session to sum up</Text>
          <Text size="lg" color="text2" align="center" style={{ marginTop: space[2] }}>Start one from Home.</Text>
        </View>
        <Cta>
          <TouchButton variant="primary" size="xl" fullWidth onPress={() => router.replace("/home")}>Done</TouchButton>
        </Cta>
      </Screen>
    );
  }

  const changes = (audit ?? []).filter((e) => e.session_id === s.sessionId);
  const lineLabel = s.line.slug === "ghost" ? "Main line" : `${s.line.name} line`;

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6], gap: space[4] }}>
        <View style={{ paddingTop: space[8], gap: space[2] }}>
          <LineGlyph slug={s.line.slug} name={s.line.name} size="xl" />
          <Text size="4xl" weight="semibold" accessibilityRole="header" style={{ marginTop: space[3] }}>Session ended</Text>
          <Text size="lg" color="text2">
            {lineLabel} · {durationWords(s.startedAt, s.endedAt)} · ended {formatTime(s.endedAt)}
          </Text>
          <Text size="lg" color="text2">The mic on this phone is off.</Text>
        </View>

        {s.endFailed && (
          <Notice tone="warn" title="Your desk didn’t get the End">
            The phone couldn’t reach it. If the session is still open at your desk, end it there.
          </Notice>
        )}

        <View>
          <SectionHead title={changes.length ? `What changed · ${changes.length}` : "What changed"} />
          {changes.length > 0 ? (
            <List>
              {changes.map((e, i) => (
                <View key={e.id}>
                  {i > 0 && <Divider />}
                  <EntryRow e={e} onPress={() => router.navigate({ pathname: "/activity", params: { open: e.id } })} />
                </View>
              ))}
            </List>
          ) : (
            <Text size="lg" color="text2">Nothing changed. Ghost only looked things up and answered.</Text>
          )}
        </View>

        {s.turns.length > 0 && (
          <Disclosure summary={`What was said · ${s.turns.length} ${s.turns.length === 1 ? "turn" : "turns"}`}>
            <View style={{ gap: space[3] }}>
              {s.turns.map((t, i) => (
                <View key={i} style={{ gap: space[0.5] }}>
                  <Text size="md" weight="medium" color="text3">{t.role === "user" ? "You" : "Ghost"}</Text>
                  <Text size="lg" color={t.role === "user" ? "text1" : "text2"}>{t.text}</Text>
                </View>
              ))}
            </View>
          </Disclosure>
        )}
      </ScrollView>

      <Cta>
        <TouchButton variant="primary" size="xl" fullWidth onPress={() => router.replace("/home")}>
          Done
        </TouchButton>
      </Cta>
    </Screen>
  );
}
