/** Paired: what this phone can now do, and the three facts that matter. */
import { useRouter } from "expo-router";
import { Check, Lock, Mic, ScrollText } from "lucide-react-native";
import type { ReactNode } from "react";
import { ScrollView, View } from "react-native";

import { usePhone } from "../../src/phone/store";
import { radius, space, useColors } from "../../src/theme";
import { TouchButton } from "../../src/ui/Button";
import { Cta, Screen } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

export default function Paired() {
  const c = useColors();
  const router = useRouter();
  const { pairing } = usePhone();
  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
        <View style={{ alignItems: "center", paddingTop: space[10], gap: space[2] }}>
          <View style={{ width: 56, height: 56, borderRadius: radius.full, backgroundColor: c.okSoft, alignItems: "center", justifyContent: "center" }}>
            <Check size={28} color={c.ok} />
          </View>
          <Text size="3xl" weight="semibold" accessibilityRole="header" style={{ marginTop: space[3] }}>Paired</Text>
          <Text size="lg" mono color="text2">{pairing?.workstation ?? ""}</Text>
          <Text size="lg" color="text2" align="center" style={{ maxWidth: 320 }}>
            This phone can start sessions with Ghost and answer its questions.
          </Text>
        </View>
        <View style={{ marginTop: space[8], gap: space[5] }}>
          <Fact icon={<Lock size={20} color={c.text2} />} lead="The mic opens only after you tap Start.">
            It closes when you tap End. Nothing listens in the background.
          </Fact>
          <Fact icon={<Mic size={20} color={c.text2} />} lead="Ghost asks out loud before it changes anything you’ve set to ask first.">
            Answer yes or no, the way you’d say it.
          </Fact>
          <Fact icon={<ScrollText size={20} color={c.text2} />} lead="Everything Ghost does is in Activity,">
            on this phone and at your desk.
          </Fact>
        </View>
      </ScrollView>
      <Cta>
        <TouchButton variant="primary" size="xl" fullWidth onPress={() => router.replace("/home")}>
          Start using Ghost
        </TouchButton>
      </Cta>
    </Screen>
  );
}

function Fact({ icon, lead, children }: { icon: ReactNode; lead: string; children: ReactNode }) {
  return (
    <View style={{ flexDirection: "row", gap: space[3] }}>
      <View style={{ paddingTop: 2 }}>{icon}</View>
      <Text size="lg" color="text2" style={{ flex: 1 }}>
        <Text size="lg" weight="semibold">{lead}</Text> {children}
      </Text>
    </View>
  );
}
