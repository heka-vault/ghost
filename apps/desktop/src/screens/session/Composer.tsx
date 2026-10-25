/**
 * Owner: session. The composer: type to Ghost, or to the line you're on, and the one voice
 * control. The frame is the field: it carries the focus ring, so the textarea inside has no
 * frame of its own. It grows from one line to six as you write (Enter sends, Shift+Enter starts
 * a new line, Escape clears), then scrolls under a soft top fade. In a live session the
 * presence sits at its leading edge, so the voice state stays in view next to the place you'd
 * type. At the trailing edge, before Send, sits the voice control: Wake Ghost (Mic) while Ghost
 * waits for its wake phrase, Hang up (PhoneOff) in a session. Waking is where audio stops being
 * local-only, so it is always a labeled button, never the logo (Dash, Gumloop, Copilot).
 *
 * The composer moves between the start state and the dock as the transcript appears or
 * clears. When it remounts, keyboard focus that was inside it comes back to the same part
 * (the field or the voice control), so Tab never restarts from <body>.
 */
import { ArrowUp } from "lucide-react";
import { forwardRef, type ReactElement, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";

import { IconButton, VoicePresence, cx, type GhostPhase } from "../../ui";

export interface VoiceControl {
  /** The accessible name and tooltip: “Wake Ghost”, “Hang up”, or why it’s off. */
  label: string;
  icon: ReactElement;
  onClick: () => void;
  disabled: boolean;
  loading: boolean;
}

export interface ComposerProps {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  disabled: boolean;
  sending: boolean;
  placeholder: string;
  label: string;
  /** The live phase, shown at the leading edge. Omit in the start state, which has its own presence. */
  presence?: GhostPhase;
  voice?: VoiceControl;
}

/** Which part of the composer held focus when it unmounted, for the one that mounts next. */
let carry: "field" | "voice" | null = null;

export const Composer = forwardRef<HTMLTextAreaElement | null, ComposerProps>(function Composer({ value, onChange, onSubmit, disabled, sending, placeholder, label, presence, voice }, ref) {
  const form = useRef<HTMLFormElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  useImperativeHandle(ref, () => field.current as HTMLTextAreaElement);
  const [multiline, setMultiline] = useState(false);
  const [scrolled, setScrolled] = useState(false);

  // Fit the height to the text; CSS caps it at six lines, after which it scrolls.
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
    // More than one line: the frame trades its pill corners for the card radius.
    setMultiline(el.scrollHeight > parseFloat(getComputedStyle(el).minHeight) + 1);
    setScrolled(el.scrollTop > 0);
  }, [value]);

  // Hand focus across a remount (start state <-> dock). Layout cleanups run before the old
  // nodes leave the document, so the focused element is still readable here.
  useLayoutEffect(() => {
    if (carry) {
      const which = carry;
      carry = null;
      const active = document.activeElement;
      if (!active || active === document.body) {
        const target = which === "voice" ? form.current?.querySelector<HTMLElement>(".session-composer__voice") : field.current;
        (target && !(target as HTMLButtonElement).disabled ? target : field.current)?.focus({ preventScroll: true });
      }
    }
    return () => {
      const active = document.activeElement;
      if (!active || !form.current?.contains(active) || !active.matches(":focus-visible")) return;
      carry = active === field.current ? "field" : active.closest(".session-composer__voice") ? "voice" : null;
    };
  }, []);

  return (
    <form
      ref={form}
      className={cx("session-composer", presence && "session-composer--live", multiline && "is-multiline", disabled && "is-disabled")}
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
    >
      {presence && <VoicePresence size={48} phase={presence} label="" className="session-composer__presence" />}
      <textarea
        ref={field}
        rows={1}
        className={cx("session-composer__input", scrolled && "is-scrolled")}
        aria-label={label}
        placeholder={placeholder}
        value={value}
        disabled={disabled}
        autoComplete="off"
        spellCheck
        enterKeyHint="send"
        onChange={(e) => onChange(e.target.value)}
        onScroll={(e) => setScrolled(e.currentTarget.scrollTop > 0)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            onSubmit();
          } else if (e.key === "Escape" && value) {
            e.preventDefault();
            onChange("");
          }
        }}
      />
      {voice && (
        <IconButton
          size="lg"
          variant="ghost"
          label={voice.label}
          icon={voice.icon}
          loading={voice.loading}
          disabled={voice.disabled}
          onClick={voice.onClick}
          className="session-composer__voice"
        />
      )}
      <IconButton type="submit" size="lg" variant="primary" label="Send" icon={<ArrowUp />} loading={sending} disabled={disabled || (!value.trim() && !sending)} className="session-composer__send" />
    </form>
  );
});
