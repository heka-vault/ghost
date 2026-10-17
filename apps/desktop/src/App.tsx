import { useGhost } from "@ghost/ui";
import { useState } from "react";

import { Abilities } from "./screens/Abilities";
import { Audit } from "./screens/Audit";
import { Chat } from "./screens/Chat";
import { Policy } from "./screens/Policy";
import { Settings } from "./screens/Settings";
import { StatusDot } from "./screens/StatusDot";

const SCREENS = {
  chat: ["Ghost", Chat],
  policy: ["What Ghost may do", Policy],
  audit: ["Audit log", Audit],
  abilities: ["Abilities", Abilities],
  settings: ["Settings", Settings],
} as const;
type Screen = keyof typeof SCREENS;

export function App() {
  const [screen, setScreen] = useState<Screen>("chat");
  const { status, connected } = useGhost();
  const Current = SCREENS[screen][1];
  return (
    <div className="app">
      <nav>
        <div className="brand">
          <StatusDot /> Ghost
        </div>
        {(Object.keys(SCREENS) as Screen[]).map((k) => (
          <button key={k} className={k === screen ? "active" : ""} onClick={() => setScreen(k)}>
            {SCREENS[k][0]}
          </button>
        ))}
        <div className="spacer" />
        <div className="muted" style={{ padding: 8, fontSize: 12 }}>
          {connected ? `ghostd ${status?.version ?? ""}` : "ghostd not running"}
        </div>
      </nav>
      <main>
        <Current />
      </main>
    </div>
  );
}
