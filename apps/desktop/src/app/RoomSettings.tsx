/**
 * Settings › General, The room: when the app goes to the room by itself (posture.ts), and the
 * floating room (ADR 0059). Kept beside this window, not in Ghost's settings: it is how this
 * desk shows Ghost, and it takes effect at once, so these are switches, not part of the page's
 * draft. The room's choices are this window's storage; the floating room's are the shell's
 * (`floating.json`), since the shell starts the surface before any window opens. Ghost on
 * stage opens the stage window, the floating room on its own backdrop for showing Ghost.
 */
import { useState } from "react";

import { Button, Card, Kbd, ListRow, Notice, SegmentedControl, Select, Switch } from "../ui";
import { inTauri } from "../transport";
import { type FloatingCaptions, type FloatingPlacement, prefOf, prefValue, useFloating, type WhileShared } from "./floating";
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
    <>
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
    <FloatingSettings />
    <StageSettings />
    </>
  );
}

/** Open the stage window: the floating room on its own backdrop (`stage.html`). */
async function openStage(): Promise<void> {
  if (!inTauri) {
    window.open("/stage.html?mock=floating", "_blank");
    return;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("stage_open").catch(() => undefined);
}

/** Ghost on stage: a window made to be shared or recorded, for showing Ghost to people. */
function StageSettings() {
  return (
    <Card list>
      <ListRow
        title="Ghost on stage"
        description="Ghost in a window of its own, on a dark stage, made for showing Ghost to people: in a meeting, share this window (it is called “Ghost · Stage”), or record it. It shows what the floating room shows, with Ghost’s words written under it for people who can’t hear it, and it never hides from a share. Also in the tray."
        trailing={
          <Button size="sm" variant="secondary" onClick={() => void openStage()}>
            Open the stage
          </Button>
        }
      />
    </Card>
  );
}

/** The floating room: on or off, which monitor, and what it can see of the desktop. */
function FloatingSettings() {
  const { status, set, error } = useFloating();
  if (!status) return null;
  const refused = status.support.kind === "no" ? status.support.why : null;
  const on = status.settings.on && !refused;
  const options = [
    { value: "other", label: "The other monitor" },
    ...status.monitors.map((m) => ({ value: `named:${m.name}`, label: `This monitor: ${m.name}${m.primary ? " (primary)" : ""}` })),
    { value: "here", label: "Where I am" },
  ];
  const value = prefValue(status.settings.monitor);
  if (!options.some((o) => o.value === value)) options.splice(1, 0, { value, label: `${value.slice(6)} (not connected)` });
  return (
    <>
      <Card list>
        <ListRow
          title="Float over the desktop"
          description={
            refused ??
            "When you call Ghost and this window isn’t the one you’re using, Ghost comes forward over your desktop instead, with no window around it, and goes back to a thin line at the bottom of the screen when you’re done. It never takes your keyboard, and clicks pass through everywhere it isn’t drawn. To send it away, say “Ghost, goodbye” or click Done under it."
          }
          trailing={<Switch aria-label="Float over the desktop" checked={on} disabled={!!refused} onChange={(v: boolean) => void set({ ...status.settings, on: v })} />}
        />
        {on && (
          <ListRow
            title="Ghost appears on"
            description={
              status.helper
                ? "Ghost knows which monitor you’re working on."
                : status.monitors.length > 1
                  ? "Without Ghost’s GNOME helper, Ghost can’t see which monitor you’re working on, so “the other monitor” is the one that isn’t primary. docs/floating-room.md says how to add the helper."
                  : "With one monitor, Ghost comes forward on the side away from your work when it can tell where that is."
            }
            trailing={<Select size="sm" className="room-select" aria-label="Ghost appears on" value={value} options={options} onChange={(e) => void set({ ...status.settings, monitor: prefOf(e.target.value) })} />}
          />
        )}
        {on && (
          <ListRow
            title="Where on the screen"
            description="Beside your work is a column at the edge away from what you’re doing; the corner is the smallest; the centre is the whole stage."
            trailing={
              <Select
                size="sm"
                className="room-select"
                aria-label="Where on the screen"
                value={status.settings.placement ?? "beside"}
                options={[
                  { value: "beside", label: "Beside your work" },
                  { value: "corner", label: "In the corner" },
                  { value: "centre", label: "In the centre" },
                ]}
                onChange={(e) => void set({ ...status.settings, placement: e.target.value as FloatingPlacement })}
              />
            }
          />
        )}
        {on && (
          <ListRow
            title="Captions"
            description="Ghost is a voice, so by default nothing it says is written on your desktop. “When the sound is off” means muted or too quiet to hear."
            trailing={
              <Select
                size="sm"
                className="room-select"
                aria-label="Captions"
                value={status.settings.captions ?? "off"}
                options={[
                  { value: "off", label: "Off" },
                  { value: "muted", label: "When the sound is off" },
                  { value: "always", label: "Always" },
                ]}
                onChange={(e) => void set({ ...status.settings, captions: e.target.value as FloatingCaptions })}
              />
            }
          />
        )}
        {on && (
          <ListRow
            title="While my screen is shared"
            description="When a meeting or recording takes your whole screen. Shown, Ghost is marked “Screen shared”; the tray changes it for one share. Sharing a single window never includes it."
            trailing={
              <Select
                size="sm"
                className="room-select"
                aria-label="While my screen is shared"
                value={status.settings.while_shared ?? "hide"}
                options={[
                  { value: "hide", label: "Hide Ghost" },
                  { value: "show", label: "Show Ghost" },
                  { value: "ask", label: "Ask each time" },
                ]}
                onChange={(e) => void set({ ...status.settings, while_shared: e.target.value as WhileShared })}
              />
            }
          />
        )}
      </Card>
      {on && status.capture === "yes" && (
        <Notice tone="info" title="Your screen is being shared or recorded">
          {status.sharing === "shown"
            ? "Ghost is in the share, marked “Screen shared”. The tray’s Show Ghost in this share takes it out."
            : status.sharing === "asking"
              ? "Ghost is asking on the screen whether to show itself in this share."
              : "Ghost stays out of it until the share ends; a spoken session shows in this window meanwhile. The tray’s Show Ghost in this share puts it in."}
        </Notice>
      )}
      {on && status.capture === "unknown" && (
        <Notice tone="warn" title="This desktop doesn’t say when the screen is shared">
          Turn Float over the desktop off from the tray before you share your screen, or Ghost may show in it.
        </Notice>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
    </>
  );
}
