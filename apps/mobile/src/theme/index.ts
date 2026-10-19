import { useColorScheme } from "react-native";

import { type Colors, colorsFor } from "./tokens";

export * from "./tokens";

const light = colorsFor("light");
const dark = colorsFor("dark");

/** The resolved colors for the phone's light or dark setting. */
export function useColors(): Colors {
  return useColorScheme() === "dark" ? dark : light;
}

export function useIsDark(): boolean {
  return useColorScheme() === "dark";
}
