/**
 * Pairing, before anything else works: scan the code your desk shows, or type its address
 * and the six-digit code. The camera is the only sensor this screen will use; the mic stays
 * off. Scanning isn't built yet (the app doesn't ask for the camera), so in a real build the
 * viewfinder says so and typing the address is the way in; on the mock desk, tap it to pair.
 */
import { useRouter } from "expo-router";
import { useState } from "react";
import { Pressable, ScrollView, View } from "react-native";

import { MOCK_DESK, pairWithMock } from "../../src/phone/pairing";
import { radius, space } from "../../src/theme";
import { GhostMark } from "../../src/ui/Brand";
import { TouchButton } from "../../src/ui/Button";
import { Badge, Cta, Screen } from "../../src/ui/parts";
import { Text } from "../../src/ui/Text";

export default function PairScan() {
  const router = useRouter();
  const [scanning, setScanning] = useState(false);

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: space[5], paddingBottom: space[6] }}>
        <View style={{ alignItems: "center", paddingTop: space[10], gap: space[3] }}>
          <GhostMark size={40} />
          <Text size="3xl" weight="semibold" align="center" accessibilityRole="header">Pair with your desk</Text>
          <Text size="lg" color="text2" align="center" style={{ maxWidth: 320 }}>
            {MOCK_DESK
              ? "On your workstation, open Ghost, go to Settings, and choose Pair a phone. Point the camera at the code it shows."
              : "On your workstation, open Ghost, go to Settings, and choose Pair a phone. Type the address and code it shows."}
          </Text>
        </View>

        {/* The camera feed is dark in both themes. */}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={MOCK_DESK ? "Camera. Mock: tap to pair with the mock desk" : "Scanning isn’t in this build yet"}
          disabled={!MOCK_DESK}
          onPress={() => {
            setScanning(true);
            setTimeout(() => {
              pairWithMock();
              router.replace("/pair/done");
            }, 900);
          }}
          style={{ marginTop: space[8], aspectRatio: 1, borderRadius: radius.xl, backgroundColor: "#0c0c0e", alignItems: "center", justifyContent: "flex-end", padding: space[5], overflow: "hidden" }}
        >
          <Corners />
          <View style={{ flexDirection: "row", alignItems: "center", gap: space[2] }}>
            <Text size="lg" style={{ color: "#ededef" }}>
              {!MOCK_DESK ? "Scanning isn’t in this build yet" : scanning ? "Pairing with workstation…" : "Tap to pair with the mock desk"}
            </Text>
            {MOCK_DESK && !scanning && <Badge tone="accent">Mock</Badge>}
          </View>
        </Pressable>
      </ScrollView>
      <Cta>
        <TouchButton variant={MOCK_DESK ? "secondary" : "primary"} size={MOCK_DESK ? "lg" : "xl"} fullWidth onPress={() => router.push("/pair/code")}>
          {MOCK_DESK ? "Enter the address instead" : "Enter the address"}
        </TouchButton>
      </Cta>
    </Screen>
  );
}

/** The viewfinder's four corner marks. */
function Corners() {
  const arm = 28;
  const w = 3;
  const color = "rgba(255, 255, 255, 0.85)";
  const at = (pos: object, edges: object) => <View style={[{ position: "absolute", width: arm, height: arm, borderColor: color }, pos, edges]} />;
  return (
    <View style={{ position: "absolute", top: "18%", left: "18%", right: "18%", bottom: "18%" }}>
      {at({ top: 0, left: 0 }, { borderTopWidth: w, borderLeftWidth: w, borderTopLeftRadius: 8 })}
      {at({ top: 0, right: 0 }, { borderTopWidth: w, borderRightWidth: w, borderTopRightRadius: 8 })}
      {at({ bottom: 0, left: 0 }, { borderBottomWidth: w, borderLeftWidth: w, borderBottomLeftRadius: 8 })}
      {at({ bottom: 0, right: 0 }, { borderBottomWidth: w, borderRightWidth: w, borderBottomRightRadius: 8 })}
    </View>
  );
}
