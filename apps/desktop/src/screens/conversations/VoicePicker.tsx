/**
 * Owner: conversations. Choose one of Kokoro's voices, grouped by accent, then female and
 * male. Auditioning is what this dialog is for, so every voice carries its own play button
 * (Higgsfield, Hume, PlayAI). The voices are native radios in one group: Tab enters at the
 * choice, arrows walk and select, P plays the focused voice on the speakers through Ghost.
 * "Play as I pick" (remembered on this computer) plays each voice as it is selected.
 */
import { Check, Play } from "lucide-react";
import { type KeyboardEvent, useEffect, useId, useMemo, useRef, useState } from "react";

import { Button, Checkbox, Dialog, IconButton, Kbd, SegmentedControl, cx } from "../../ui";
import { type Voice, ENGLISH, groupVoices, voiceKind } from "./voices";

export interface VoicePickerProps {
  open: boolean;
  onClose: () => void;
  lineName: string;
  voices: Voice[];
  value: string;
  onPick: (id: string) => void;
  /** null when Hear it is available, otherwise why it isn't. */
  hearBlocked: string | null;
  /** The voice playing a sample now, if any. */
  hearing: string | null;
  onHear: (id: string) => void;
}

type Bank = "english" | "other";

const AUTOPLAY_KEY = "ghost.conversations.playOnPick";
const readAutoplay = () => {
  try {
    return window.localStorage.getItem(AUTOPLAY_KEY) === "1";
  } catch {
    return false;
  }
};

/** A name alone is ambiguous across languages (Dora, Alex and Santa are each in two). */
export function voiceTitle(v: Voice, all: Voice[]): string {
  return all.filter((x) => x.name === v.name).length > 1 ? `${v.name}, ${v.accent}` : v.name;
}

/** Three small bars that move while a sample plays (still under reduced motion). */
export function PlayingBars() {
  return (
    <span className="conv-bars" aria-hidden>
      <span />
      <span />
      <span />
    </span>
  );
}

export function VoicePicker({ open, onClose, lineName, voices, value, onPick, hearBlocked, hearing, onHear }: VoicePickerProps) {
  const name = useId();
  const [chosen, setChosen] = useState(value);
  const current = voices.find((v) => v.id === chosen);
  const [bank, setBank] = useState<Bank>("english");
  const [autoplay, setAutoplayState] = useState(readAutoplay);
  const setAutoplay = (on: boolean) => {
    setAutoplayState(on);
    try {
      window.localStorage.setItem(AUTOPLAY_KEY, on ? "1" : "0");
    } catch {
      /* private mode: the choice lasts for this dialog */
    }
  };

  const groupsRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    setChosen(value);
    const v = voices.find((x) => x.id === value);
    setBank(v && !ENGLISH.has(v.accent) ? "other" : "english");
    // Start on the current voice, not the close button.
    const id = requestAnimationFrame(() => {
      const box = groupsRef.current;
      const el = box?.querySelector<HTMLInputElement>("input:checked");
      el?.focus({ preventScroll: true });
      // Scroll the list itself, never scrollIntoView (it moves Chrome's Tab starting point).
      const cellEl = el?.closest<HTMLElement>(".conv-vc");
      if (box && cellEl) {
        const b = box.getBoundingClientRect();
        const r = cellEl.getBoundingClientRect();
        const pad = 8;
        if (r.top < b.top) box.scrollTop += r.top - b.top - pad;
        else if (r.bottom > b.bottom) box.scrollTop += r.bottom - b.bottom + pad;
      }
    });
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const groups = useMemo(() => groupVoices(voices).filter((g) => ENGLISH.has(g.accent) === (bank === "english")), [voices, bank]);
  const counts = useMemo(() => {
    const en = voices.filter((v) => ENGLISH.has(v.accent)).length;
    return { english: en, other: voices.length - en };
  }, [voices]);
  const chosenInBank = !!current && ENGLISH.has(current.accent) === (bank === "english");

  const pick = (id: string) => {
    setChosen(id);
    if (autoplay && !hearBlocked) onHear(id);
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>, v: Voice) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === "p" || e.key === "P") {
      e.preventDefault();
      if (!hearBlocked) onHear(v.id);
    }
  };

  const cell = (v: Voice) => {
    const on = v.id === chosen;
    const playing = hearing === v.id;
    const title = voiceTitle(v, voices);
    return (
      <div key={v.id} className={cx("conv-vc", on && "is-on", playing && "is-playing")}>
        <label className="conv-vc__pick">
          <input
            type="radio"
            className="conv-vc__input"
            name={name}
            value={v.id}
            checked={on}
            aria-keyshortcuts="p"
            aria-describedby={`${name}-hint`}
            onChange={() => pick(v.id)}
            onKeyDown={(e) => onKey(e, v)}
          />
          {on && <Check size={14} aria-hidden className="conv-vc__mark" />}
          <span className="conv-vc__name">{v.name}</span>
          <span className="sr-only">, {voiceKind(v)}</span>
        </label>
        {/* Out of the Tab order: 54 more stops would bury Cancel and Use. P plays from the keyboard. */}
        <IconButton
          size="sm"
          tabIndex={-1}
          className="conv-vc__play"
          label={playing ? `Playing ${title}` : `Hear ${title}`}
          icon={playing ? <PlayingBars /> : <Play size={14} />}
          disabled={!!hearBlocked || (hearing != null && !playing)}
          onClick={() => onHear(v.id)}
        />
      </div>
    );
  };

  const status = hearBlocked
    ? hearBlocked
    : hearing
      ? `Playing ${voiceTitle(voices.find((v) => v.id === hearing) ?? { id: hearing, name: hearing, accent: "", sex: "other" }, voices)} on your speakers…`
      : null;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title={`A voice for ${lineName}`}
      description="Kokoro voices, running on this computer. Each plays on your speakers, so you can hear it before you use it."
      footer={
        <div className="conv-picker__foot">
          <div className="conv-picker__chosen">
            <p className="t-body t-1">
              {current ? (
                <>
                  <span className="t-medium">{current.name}</span> · {voiceKind(current)}
                  {!chosenInBank && <span className="t-2"> · in {bank === "english" ? "other languages" : "English"}</span>}
                </>
              ) : (
                "Pick a voice"
              )}
            </p>
            <p id={`${name}-hint`} className="conv-picker__hint t-small t-3" aria-live="polite">
              {status ?? (
                <>
                  Arrows to move, <Kbd>P</Kbd> to hear the focused voice
                </>
              )}
            </p>
          </div>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!current}
            onClick={() => {
              onPick(chosen);
              onClose();
            }}
          >
            Use {current ? voiceTitle(current, voices) : "this voice"}
          </Button>
        </div>
      }
    >
      <div className="conv-picker__bar">
        <SegmentedControl<Bank>
          aria-label="Language"
          size="sm"
          value={bank}
          onChange={setBank}
          options={[
            { value: "english", label: `English · ${counts.english}` },
            { value: "other", label: `Other languages · ${counts.other}` },
          ]}
        />
        <Checkbox checked={autoplay} onChange={setAutoplay} disabled={!!hearBlocked} label="Play as I pick" />
      </div>
      <div ref={groupsRef} className="conv-picker__groups" role="radiogroup" aria-label={`Voices for ${lineName}`}>
        {bank === "other" ? (
          // Few voices per accent: one row each, female and male side by side, so the bank
          // fits without scrolling. Column heads are visual; each voice says its kind itself.
          <div className="conv-picker__table">
            <span aria-hidden />
            <span className="conv-picker__col" aria-hidden>
              Female
            </span>
            <span className="conv-picker__col" aria-hidden>
              Male
            </span>
            {groups.map((g) => (
              <section key={g.accent} className="conv-picker__trow" aria-label={g.accent}>
                <h3 className="conv-picker__accent conv-picker__accent--row">{g.accent}</h3>
                <div className="conv-picker__cells conv-picker__cells--half">{[...g.female, ...g.other].map(cell)}</div>
                <div className="conv-picker__cells conv-picker__cells--half">{g.male.map(cell)}</div>
              </section>
            ))}
          </div>
        ) : groups.map((g) => (
          <section key={g.accent} className="conv-picker__group" aria-label={g.accent}>
            <h3 className="conv-picker__accent">{g.accent}</h3>
            {(["female", "male", "other"] as const).map((sex) =>
              g[sex].length ? (
                <div key={sex} className="conv-picker__row">
                  <span className="conv-picker__sex">{sex === "other" ? "Voices" : sex === "female" ? "Female" : "Male"}</span>
                  <div className="conv-picker__cells">{g[sex].map(cell)}</div>
                </div>
              ) : null,
            )}
          </section>
        ))}
      </div>
    </Dialog>
  );
}
