import { Redirect } from "expo-router";
import { type BottomTabBarProps, Tabs } from "expo-router/tabs";
import { House, ScrollText, Settings2 } from "lucide-react-native";
import { Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { usePhone } from "../../src/phone/store";
import { radius, space, useColors } from "../../src/theme";
import { Text } from "../../src/ui/Text";

const TABS = { home: { label: "Home", icon: House }, activity: { label: "Activity", icon: ScrollText }, settings: { label: "Settings", icon: Settings2 } } as const;

export default function TabsLayout() {
  const { pairing } = usePhone();
  if (!pairing) return <Redirect href="/pair" />;
  return (
    <Tabs screenOptions={{ headerShown: false }} tabBar={(p) => <TabBar {...p} />}>
      <Tabs.Screen name="home" />
      <Tabs.Screen name="activity" />
      <Tabs.Screen name="settings" />
    </Tabs>
  );
}

/** Material-style bottom navigation: three destinations, 72dp, the current one on a pill. */
function TabBar({ state, navigateToTab }: BottomTabBarProps) {
  const c = useColors();
  const insets = useSafeAreaInsets();
  return (
    <View accessibilityRole="tablist" style={{ flexDirection: "row", height: space[16] + space[2] + insets.bottom, paddingBottom: insets.bottom, borderTopWidth: 1, borderTopColor: c.borderSubtle, backgroundColor: c.bgCanvas }}>
      {state.routes.map((route, i) => {
        const tab = TABS[route.name as keyof typeof TABS];
        if (!tab) return null;
        const current = state.index === i;
        const Icon = tab.icon;
        return (
          <Pressable
            key={route.key}
            accessibilityRole="tab"
            accessibilityState={{ selected: current }}
            accessibilityLabel={tab.label}
            onPress={() => {
              if (!current) navigateToTab(route.key);
            }}
            style={{ flex: 1, alignItems: "center", justifyContent: "center", gap: space[1] }}
          >
            {({ pressed }) => (
              <>
                <View style={{ width: space[16], height: space[8], borderRadius: radius.full, alignItems: "center", justifyContent: "center", backgroundColor: current ? c.bgSelected : pressed ? c.bgActive : "transparent" }}>
                  <Icon size={20} color={current ? c.text1 : c.text2} />
                </View>
                <Text size="sm" weight={current ? "semibold" : "medium"} color={current ? "text1" : "text2"}>
                  {tab.label}
                </Text>
              </>
            )}
          </Pressable>
        );
      })}
    </View>
  );
}
