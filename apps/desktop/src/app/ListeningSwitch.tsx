/**
 * The switch for Ghost's microphone (ADR 0055), as a call app's mute: the mic glyph while
 * Ghost listens, the struck-through mic in red while it doesn't. Turning it off asks how
 * long (do-not-disturb's choices); turning it on is one click. Ctrl+Shift+M anywhere in the
 * app does either, off until turned back on.
 */
import { useGhost } from "@ghost/ui";
import { Mic, MicOff } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Button, IconButton, Menu, type MenuItem, cx, errorText, formatShortcut, useLive, useToast } from "../ui";
import { LISTENING_SHORTCUT, OFF_FOR, notListening, switchLabel } from "./listening";

/** The switch's state and its two moves, shared by the sidebar, Settings and the shortcut. */
export function useListening() {
  const { client, status } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const off = notListening(status);
  const set = useCallback(
    async (listening: boolean, minutes?: number) => {
      setBusy(true);
      try {
        await client.setListening(listening, minutes);
      } catch (e) {
        toast({ tone: "danger", title: listening ? "Couldn’t turn listening on" : "Couldn’t stop listening", description: errorText(e) });
      } finally {
        setBusy(false);
      }
    },
    [client, toast],
  );
  return { off, online, busy, set };
}

/** Ctrl+Shift+M in the app: off until turned back on, or on again. Mounted once, by the sidebar. */
export function useListeningShortcut() {
  const { off, online, set } = useListening();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.repeat || e.altKey || !e.shiftKey || !(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "m") return;
      if (!online) return;
      e.preventDefault();
      void set(!!off);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [off, online, set]);
}

function offItems(set: (listening: boolean, minutes?: number) => void): MenuItem[] {
  return [{ heading: "Stop listening" }, ...OFF_FOR.map((o) => ({ label: o.label, onSelect: () => set(false, o.minutes), shortcut: o.minutes ? undefined : LISTENING_SHORTCUT }))];
}

/** The icon switch beside the sidebar's status card. */
export function ListeningSwitch({ rail = false }: { rail?: boolean }) {
  const { off, online, busy, set } = useListening();
  const label = switchLabel(off);
  const tip = `${label} (${formatShortcut(LISTENING_SHORTCUT)})`;
  if (off) {
    return (
      <IconButton
        className={cx("listening-switch", "is-off")}
        label={tip}
        icon={<MicOff />}
        pressed
        loading={busy}
        disabled={!online}
        tooltipSide={rail ? "right" : "bottom"}
        onClick={() => void set(true)}
      />
    );
  }
  return (
    <Menu
      align="end"
      aria-label="Stop listening"
      items={offItems((l, m) => void set(l, m))}
      trigger={<IconButton className="listening-switch" label={tip} icon={<Mic />} loading={busy} disabled={!online} tooltipSide={rail ? "right" : "bottom"} />}
    />
  );
}

/** Settings' row: the same switch, in words. */
export function ListeningButton() {
  const { off, online, busy, set } = useListening();
  if (off) {
    return (
      <Button size="sm" variant="secondary" leadingIcon={<Mic />} loading={busy} disabled={!online} onClick={() => void set(true)}>
        Listen again
      </Button>
    );
  }
  return <Menu align="end" items={offItems((l, m) => void set(l, m))} trigger={<Button size="sm" variant="secondary" leadingIcon={<MicOff />} loading={busy} disabled={!online}>Stop listening…</Button>} />;
}
