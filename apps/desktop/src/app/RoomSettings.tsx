/**
 * Settings › General, The room: when the app goes to the room by itself (posture.ts). Kept in
 * this window's own storage: it is how this screen behaves on this desk, not Ghost's setting,
 * and it takes effect at once, so these are switches, not part of the page's draft.
 */
import { useState } from "react";

import { Button, Card, Kbd, ListRow, SegmentedControl, Switch } from "../ui";
import { AWAY_CHOICES, readAway, readSessionRoom, writeAway, writeSessionRoom } from "./posture";
import { usePosture } from "./PostureContext";

const AWAY_OPTIONS = [...AWAY_CHOICES.map((m) => ({ value: String(m), label: `${m} min` })), { value: "never", label: "Never" }];

export function RoomSettings() {
  const { refresh, hold } = usePosture();
  const [away, setAway] = useState<string>(() => {
    const v = readAway();
    return v == null ? "never" : String(v);
  });
  const [session, setSession] = useState(readSessionRoom);
  return (
    <Card list>
      <ListRow
        title="When you step away"
        description="With nothing touched for this long, Ghost fills the window: what needs you in large type, and what it is doing. A key or the mouse brings the desk back."
        trailing={
          <SegmentedControl
            size="sm"
            aria-label="Go to the room after"
            options={AWAY_OPTIONS}
            value={away}
            onChange={(v) => {
              setAway(v);
              writeAway(v === "never" ? null : Number(v));
              refresh();
            }}
          />
        }
      />
      <ListRow
        title="In a spoken session"
        description="Twenty seconds after you last touched the computer in a conversation, the conversation goes to room view."
        trailing={
          <Switch
            aria-label="Room view in a spoken session"
            checked={session}
            onChange={(on: boolean) => {
              setSession(on);
              writeSessionRoom(on);
              refresh();
            }}
          />
        }
      />
      <ListRow
        title="Go to the room now"
        description={
          <>
            <Kbd shortcut="mod+." /> from anywhere; Escape or the same keys come back.
          </>
        }
        trailing={
          <Button size="sm" variant="secondary" onClick={() => hold("room")}>
            Show the room
          </Button>
        }
      />
    </Card>
  );
}
