/** Owner: settings. Page-local pieces: chips input, secret row, save bar, setup list. */
import type { Status } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Check, CircleCheck, Copy, X } from "lucide-react";
import { type ClipboardEvent, type KeyboardEvent, type ReactNode, type Ref, useEffect, useRef, useState } from "react";

import { Button, Card, IconButton, ListRow, SaveBar as SharedSaveBar, Section, Textarea, cx, useFieldControl } from "../../ui";
import { type SectionId, SECTION_TITLE, joinList, listWords, lowerFirst, sectionDomId, splitLines, splitList } from "./model";

/* ------------------------------------------------------------------- ChipsInput */
export interface ChipsInputProps {
  items: string[];
  onChange: (items: string[]) => void;
  placeholder: string;
  /** Returns an error for an entry that can't be added, or null. */
  check?: (entry: string) => string | null;
  onEntryError?: (msg: string | null) => void;
  /** How a chip reads: identifiers in mono, spoken phrases in typographer's quotes. */
  kind: "mono" | "phrase";
  /** "repository", "phrase": names the remove buttons. */
  noun: string;
  disabled?: boolean;
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean;
}

/**
 * A list edited as chips: type and press Enter or a comma; Backspace on empty removes the last.
 * One Tab stop for the whole group (the text input). Left from the start of the input walks
 * back over the chips; Left and Right move between them, Delete or Backspace removes the one
 * in focus, Enter or Space on a chip removes it too. Adds and removes are announced.
 */
export function ChipsInput({ items, onChange, placeholder, check, onEntryError, kind, noun, disabled, ...rest }: ChipsInputProps) {
  const a = useFieldControl(rest);
  const [text, setText] = useState("");
  const [said, setSaid] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [invalid, setInvalid] = useState(false);
  // Where focus should land once the list has re-rendered after a removal: a chip index, or the input.
  const after = useRef<number | "input" | null>(null);
  useEffect(() => {
    const t = after.current;
    if (t == null) return;
    after.current = null;
    if (t === "input" || !items.length) input.current?.focus({ preventScroll: true });
    else chipButton(Math.min(t, items.length - 1))?.focus({ preventScroll: true });
  });

  const chipButton = (i: number) => list.current?.querySelectorAll<HTMLButtonElement>(".set-chip__remove")[i] ?? null;
  const say = (what: string) => setSaid(what === said ? `${what} ` : what);

  const add = (raw: string) => {
    const entries = splitList(raw);
    if (!entries.length) return true;
    for (const e of entries) {
      const err = check?.(e) ?? null;
      if (err) {
        setInvalid(true);
        onEntryError?.(err);
        return false;
      }
    }
    const next = [...items];
    const added: string[] = [];
    for (const e of entries)
      if (!next.some((n) => n.toLowerCase() === e.toLowerCase())) {
        next.push(e);
        added.push(e);
      }
    onChange(next);
    setText("");
    setInvalid(false);
    onEntryError?.(null);
    if (added.length) say(`Added ${added.join(", ")}`);
    return true;
  };
  const remove = (i: number, focusNext: number | "input") => {
    say(`Removed ${items[i]}`);
    onChange(items.filter((_, j) => j !== i));
    after.current = focusNext;
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" || e.key === ",") {
      if (text.trim()) {
        e.preventDefault();
        add(text);
      } else if (e.key === ",") e.preventDefault();
      // An empty Enter falls through and submits the form (Save), like every other field.
    } else if (e.key === "Backspace" && !text && items.length) {
      e.preventDefault();
      remove(items.length - 1, "input");
    } else if (e.key === "ArrowLeft" && items.length && e.currentTarget.selectionStart === 0 && e.currentTarget.selectionEnd === 0) {
      e.preventDefault();
      chipButton(items.length - 1)?.focus();
    }
  };
  const onChipKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    if (e.key === "ArrowLeft" && i > 0) {
      e.preventDefault();
      chipButton(i - 1)?.focus();
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      if (i < items.length - 1) chipButton(i + 1)?.focus();
      else input.current?.focus();
    } else if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      remove(i, i < items.length - 1 ? i : "input");
    } else if (e.key === "Home") {
      e.preventDefault();
      chipButton(0)?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      input.current?.focus();
    } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // Typing from a chip goes to the input, keeping the character.
      input.current?.focus();
    }
  };
  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const t = e.clipboardData.getData("text");
    if (t.includes(",") || t.includes("\n")) {
      e.preventDefault();
      add(t.replace(/\n/g, ","));
    }
  };

  return (
    <div
      className={cx("set-chips", (invalid || a["aria-invalid"]) && "is-invalid", disabled && "is-disabled")}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) {
          e.preventDefault();
          input.current?.focus();
        }
      }}
    >
      <ul ref={list} className="set-chips__list" aria-label={`${items.length} ${noun}${items.length === 1 ? "" : "s"}`}>
        {items.map((item, i) => (
          <li key={item} className={cx("set-chip", kind === "mono" && "is-mono")} data-user-content>
            <span className="set-chip__text">{kind === "phrase" ? `“${item}”` : item}</span>
            <button
              type="button"
              className="set-chip__remove"
              tabIndex={-1}
              aria-label={`Remove ${item}`}
              disabled={disabled}
              onKeyDown={(e) => onChipKey(e, i)}
              onClick={() => remove(i, i < items.length - 1 ? i : "input")}
            >
              <X size={12} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      <input
        ref={input}
        {...a}
        aria-invalid={invalid || a["aria-invalid"] || undefined}
        className={cx("set-chips__input", kind === "mono" && "is-mono")}
        value={text}
        disabled={disabled}
        placeholder={items.length ? `Add ${noun === "repository" ? "another" : "a phrase"}…` : placeholder}
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          if (invalid) {
            setInvalid(false);
            onEntryError?.(null);
          }
        }}
        onKeyDown={onKey}
        onPaste={onPaste}
        onBlur={() => {
          if (text.trim()) add(text);
        }}
      />
      <span className="sr-only" aria-live="polite">
        {said}
      </span>
    </div>
  );
}

/* -------------------------------------------------------------------- LinesInput */
export interface LinesInputProps {
  /** The stored form: entries separated by commas. */
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  rows?: number;
  disabled?: boolean;
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean;
}

/**
 * A comma-separated setting edited one entry per line (paths, which may hold spaces but never
 * commas). The text stays local while typing, so a line break survives the render that
 * follows it; the value goes out as the comma list. A value changed from outside (Discard, a
 * reload) resets the text.
 */
export function LinesInput({ value, onChange, placeholder, rows = 3, disabled, ...rest }: LinesInputProps) {
  const [text, setText] = useState(() => splitList(value).join("\n"));
  const sent = useRef(value);
  useEffect(() => {
    if (value === sent.current) return;
    sent.current = value;
    if (joinList(splitLines(text)) !== value) setText(splitList(value).join("\n"));
  }, [value, text]);
  return (
    <Textarea
      {...rest}
      mono
      rows={rows}
      value={text}
      placeholder={placeholder}
      disabled={disabled}
      spellCheck={false}
      onChange={(e) => {
        const t = e.target.value;
        setText(t);
        const next = joinList(splitLines(t));
        sent.current = next;
        onChange(next);
      }}
    />
  );
}

/* --------------------------------------------------------------- password rows */
/** A password row's Add…/Replace… button id (Passwords.tsx), so the page can return focus to it
 *  and a link elsewhere ("Add one under Passwords") can land on it. */
export const secretActionId = (name: string) => `set-secret-${name}-action`;

/* ---------------------------------------------------------------------- SaveBar */
export type BarState =
  | {
      kind: "dirty";
      keys: string[];
      online: boolean;
      busy: boolean;
      error: string | null;
      /** Every changed key that can't be saved yet, in page order. */
      invalid: string[];
      /** The draft came back from sessionStorage after leaving the page. */
      restored: boolean;
    }
  | {
      kind: "restart";
      /** Saved keys Ghost reads only at its next start. */
      waiting: string[];
      /** Keys from the same save that are live already. */
      live: string[];
    };

export const RESTART_COMMAND = "systemctl --user restart ghostd";
/** A checkout's daemon (make dev): restarting the service instead would start a second Ghost. */
export const DEV_RESTART_COMMAND = "make reload";

/** How to restart the Ghost this window talks to, by its build (docs/releasing.md): `make reload`
 *  in the checkout for a dev build, the service for a deploy or a release. Unknown: the service. */
export function restartCommand(build: Status["build"] | null | undefined): string {
  return build?.kind === "dev" ? DEV_RESTART_COMMAND : RESTART_COMMAND;
}

/** The restart command for the running Ghost. */
export function useRestartCommand(): string {
  return restartCommand(useGhost().status?.build);
}

/** A command as plain mono text with Copy beside it; the restart command by default. Used by the
 *  pill and by rows that owe a restart. */
export function CommandCopy({ command, copyRef, className }: { command?: string; copyRef?: Ref<HTMLButtonElement>; className?: string }) {
  const restart = useRestartCommand();
  command ??= restart;
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <span className={cx("set-command", className)}>
      <code>{command}</code>
      <IconButton
        ref={copyRef}
        size="sm"
        label={copied ? "Copied" : "Copy command"}
        icon={copied ? <Check /> : <Copy />}
        onClick={() => {
          navigator.clipboard?.writeText(command).then(() => setCopied(true), () => {});
        }}
      />
    </span>
  );
}

export interface SaveBarProps {
  state: BarState;
  onSave: () => void;
  onDiscard: () => void;
  onDismiss: () => void;
  onFix: (key: string) => void;
  /** The restart variant's Copy button, so focus can land on it when it replaces the dirty bar. */
  copyRef?: Ref<HTMLButtonElement>;
  saveRef?: Ref<HTMLButtonElement>;
}

/** Up to three keys by name; beyond that, a count. */
function fewNames(keys: string[]): string {
  return keys.length <= 3 ? listWords(keys) : `${keys.length} changes`;
}

/**
 * The page's one dock, on the shared SaveBar. Unsaved changes say how many and which, then
 * Discard and Save. After a save, the same pill leads with the one thing left to do, "Restart
 * Ghost to apply", and names on its second line what waits and what is live already ("After
 * the restart: model endpoint and GitHub token. Live now: wake phrases."), counts only beyond
 * three keys, with the command and Copy. No verb, so a plural label ("wake phrases",
 * "repositories") never clashes with a singular one; the second line wraps instead of
 * truncating, the way a failed save's reason does. Each row says "Live now." or "Waits for a
 * restart." itself. A save that failed keeps the pill (red edge) and puts the reason on that
 * same second line: one surface, one edge.
 */
export function SaveBar({ state, onSave, onDiscard, onDismiss, onFix, copyRef, saveRef }: SaveBarProps) {
  const restart = useRestartCommand();
  if (state.kind === "restart") {
    const names = `After the restart: ${lowerFirst(fewNames(state.waiting))}.${state.live.length ? ` Live now: ${lowerFirst(fewNames(state.live))}.` : ""}`;
    return <SharedSaveBar variant="restart" title="Restart Ghost to apply" description={names} command={restart} copyRef={copyRef} onDismiss={onDismiss} />;
  }

  const { keys, online, busy, error, invalid, restored } = state;
  const n = keys.length;
  const count = `${n} unsaved change${n === 1 ? "" : "s"}`;
  const names = lowerFirst(listWords(keys));
  const [lead, rest] = !online
    ? [count, "they wait here until Ghost is back"]
    : invalid.length === 1
      ? ["Fix 1 field to save", lowerFirst(listWords(invalid))]
      : invalid.length > 1
        ? [`${invalid.length} fields need a fix`, lowerFirst(listWords(invalid))]
        : error
          ? ["Couldn’t save", names]
          : restored
            ? [count, `kept from before you left: ${names}`]
            : [count, names];
  return (
    <SharedSaveBar
      variant={error || invalid.length > 0 ? "error" : !online ? "offline" : "dirty"}
      title={lead}
      detail={rest}
      description={error ?? undefined}
      busy={busy}
      saveRef={saveRef}
      onDiscard={onDiscard}
      onSave={() => (invalid.length ? onFix(invalid[0]) : onSave())}
    />
  );
}

/* ----------------------------------------------------------------- SetupSection */
export interface SetupStep {
  id: string;
  title: string;
  done: boolean;
  doneText: string;
  todoText: string;
  target: SectionId;
}

/** First run: the things between a fresh install and “Hey Ghost”, leading General. Each step goes to its page. */
export function SetupSection({ steps, onGo }: { steps: SetupStep[]; onGo: (id: SectionId) => void }) {
  const done = steps.filter((s) => s.done).length;
  return (
    <Section
      id={sectionDomId("setup")}
      className="set-section"
      title={
        <span className="set-title-tag">
          {SECTION_TITLE.setup}
          <span className="set-count t-num">
            {done} of {steps.length} done
          </span>
        </span>
      }
      description={`${steps.length === 2 ? "Two" : "Three"} things between a fresh install and “Hey Ghost”.`}
    >
      <Card list>
        {steps.map((s) => (
          <ListRow
            key={s.id}
            onClick={() => onGo(s.target)}
            chevron
            leading={s.done ? <CircleCheck className="set-step__done" aria-label="Done" /> : <span className="set-step__todo" aria-label="To do" />}
            title={<span className={cx(s.done && "set-step__title-done")}>{s.title}</span>}
            description={s.done ? s.doneText : s.todoText}
          />
        ))}
      </Card>
    </Section>
  );
}

/* ---------------------------------------------------------------------- Helpers */
/** A labeled block inside a card row, for content that isn't a single control. */
export function Row({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("set-row", className)}>{children}</div>;
}
