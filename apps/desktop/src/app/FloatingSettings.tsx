/**
 * Settings › General, The floating room (ADR 0059): where Ghost appears when you call it, as
 * one choice: floating over your work, or in this window. Since 2026-11-14 the only way Ghost
 * fills more than its strip: the in-window room is gone. Kept beside this window, not in
 * Ghost's settings: the floating room's choices are the shell's (`floating.json`), since the
 * shell starts the surface before any window opens, and take effect at once, so these are
 * switches, not part of the page's draft. Captions are a choice inside floating. Ghost on
 * stage (the stage window, for showing Ghost to people) is the tray's item (ADR 0072).
 */
import { Card, ListRow, Notice, SegmentedControl, Select } from "../ui";
import { type FloatingCaptions, type FloatingPlacement, prefOf, prefValue, useFloating, type WhileShared } from "./floating";

/** The section's body. */
export function FloatingRoomSettings() {
  return <FloatingSettings />;
}

/** Where Ghost appears: floating, with its choices, or in the window. */
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
          title="Where Ghost appears"
          description={
            refused ??
            (on
              ? "When you call Ghost and this window isn’t the one you’re using, Ghost comes forward over your desktop instead, with no window around it, and goes back to a thin line at the bottom of the screen when you’re done. It never takes your keyboard, and clicks pass through everywhere it isn’t drawn. To send it away, say “Ghost, goodbye” or click Done under it."
              : "In this window: Today’s talk bar becomes a live strip while you talk, with a yes or a question and its buttons.")
          }
          trailing={
            <SegmentedControl
              aria-label="Where Ghost appears"
              value={on ? "float" : "window"}
              onChange={(v: string) => void set({ ...status.settings, on: v === "float" })}
              options={[
                { value: "float", label: "Over your work", disabled: !!refused },
                { value: "window", label: "In the window" },
              ]}
            />
          }
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
            trailing={<Select size="sm" className="float-select" aria-label="Ghost appears on" value={value} options={options} onChange={(e) => void set({ ...status.settings, monitor: prefOf(e.target.value) })} />}
          />
        )}
        {on && (
          <ListRow
            title="Where on the screen"
            description="Beside your work is a column at the edge away from what you’re doing; the corner is the smallest; the centre is the whole stage."
            trailing={
              <Select
                size="sm"
                className="float-select"
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
                className="float-select"
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
                className="float-select"
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
