/** Owner: settings. Page-local pieces: chips input, secret row, save bar, section rail, setup list. */
import { Check, CircleCheck, Copy, KeyRound, RotateCw, X } from "lucide-react";
import { type ClipboardEvent, type FormEvent, type KeyboardEvent, type ReactNode, type Ref, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, IconButton, Input, ListRow, SaveBar as SharedSaveBar, Section, Textarea, cx, useFieldControl } from "../../ui";
import { KEY_LABEL, SECRET_INFO, type SectionId, SECTION_TITLE, joinList, listWords, lowerFirst, sectionDomId, splitLines, splitList } from "./model";

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

/* -------------------------------------------------------------------- SecretRow */
export interface SecretRowProps {
  name: string;
  set: boolean;
  value: string | undefined;
  open: boolean;
  onOpen: () => void;
  onChange: (v: string) => void;
  onCancel: () => void;
  onSubmit: (e: FormEvent) => void;
  disabled?: boolean;
  /** Saved, but Ghost reads it only at its next start. */
  waiting?: boolean;
}

/** The Replace…/Add… button's id, so the page can return focus to it after a save closes the row. */
export const secretActionId = (name: string) => `set-secret-${name}-action`;

/**
 * One secret: its name, what it is for, and Set or Not set. Never a value, never a mask.
 * "Replace…" opens a password field in place; the value goes out with Save and is forgotten.
 * The badge keeps saying Set or Not set while a value is typed (that is the fact the operator
 * is deciding on); the help line says what Save will do with the typed one.
 * Closing the field (Cancel, Escape) puts focus back on the button that opened it.
 */
export function SecretRow({ name, set, value, open, onOpen, onChange, onCancel, onSubmit, disabled, waiting }: SecretRowProps) {
  const info = SECRET_INFO[name] ?? { help: "", placeholder: "Paste the value" };
  const label = KEY_LABEL[name] ?? name;
  const typed = !!value;
  const inputId = `set-secret-${name}`;
  const helpId = `${inputId}-help`;
  const action = useRef<HTMLButtonElement>(null);
  const cancel = () => {
    onCancel();
    requestAnimationFrame(() => action.current?.focus({ preventScroll: true }));
  };
  // Required and missing is the one secret that asks for attention; everything else stays quiet.
  const variant = !set && !info.optional ? "secondary" : "ghost";
  return (
    <form className={cx("set-secret", open && "is-open")} onSubmit={onSubmit} autoComplete="off" aria-label={label}>
      <div className="set-secret__main">
        <div className="set-secret__text">
          <p className="set-secret__label">
            {open ? <label htmlFor={inputId}>{label}</label> : <span>{label}</span>}
            <span className="set-detail">{name}</span>
          </p>
          <p className="set-help" id={helpId}>
            {typed ? (
              <span className="set-help-strong">{set ? "Replaces the saved value on Save. " : "Saved with your other changes. "}</span>
            ) : (
              waiting && <span className="set-waits">Waits for a restart. </span>
            )}
            {info.help}
          </p>
        </div>
        <div className="set-secret__state">
          {set ? (
            <Badge tone="ok" size="sm" dot>Set</Badge>
          ) : (
            <Badge size="sm">Not set</Badge>
          )}
          <span className="set-secret__action">
            {open ? (
              <Button key="cancel" size="sm" variant="ghost" className="set-secret__btn" onClick={cancel} aria-label={`Cancel ${label}`}>
                Cancel
              </Button>
            ) : (
              <Button
                key="open"
                ref={action}
                id={secretActionId(name)}
                size="sm"
                variant={variant}
                className="set-secret__btn"
                onClick={onOpen}
                disabled={disabled}
                aria-label={`${set ? "Replace" : "Add"} ${label}`}
              >
                {set ? "Replace…" : "Add…"}
              </Button>
            )}
          </span>
        </div>
      </div>
      {open && (
        <div className="set-secret__edit">
          <div className="set-secret__field">
            <Input
              id={inputId}
              type="password"
              autoComplete="off"
              autoFocus
              spellCheck={false}
              mono
              leading={<KeyRound size={14} aria-hidden />}
              placeholder={info.placeholder}
              aria-describedby={helpId}
              value={value ?? ""}
              onChange={(e) => onChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  cancel();
                }
              }}
            />
          </div>
        </div>
      )}
    </form>
  );
}

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

/** The restart command as plain mono text with Copy beside it. Used by the pill and by rows that owe a restart. */
export function CommandCopy({ command = RESTART_COMMAND, copyRef, className }: { command?: string; copyRef?: Ref<HTMLButtonElement>; className?: string }) {
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
  if (state.kind === "restart") {
    const names = `After the restart: ${lowerFirst(fewNames(state.waiting))}.${state.live.length ? ` Live now: ${lowerFirst(fewNames(state.live))}.` : ""}`;
    return <SharedSaveBar variant="restart" title="Restart Ghost to apply" description={names} command={RESTART_COMMAND} copyRef={copyRef} onDismiss={onDismiss} />;
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

/* ---------------------------------------------------------------------- PageNav */
export interface NavItem {
  id: SectionId;
  label: string;
  /** A small mark after the label: unsaved changes, or a restart still owed. */
  mark?: "dirty" | "restart" | "error";
  prototype?: boolean;
}

const MARK_LABEL = { dirty: "unsaved changes", restart: "restart to apply", error: "needs a fix" };

/**
 * The "on this page" rail to the right of the column (Neon). Tracks the section in view. One
 * Tab stop (the current section, like Tabs and SegmentedControl): ArrowUp/Down and Home/End
 * move between sections, Enter or Space goes there.
 */
export function PageNav({ items, active, onGo }: { items: NavItem[]; active: SectionId | null; onGo: (id: SectionId) => void }) {
  const list = useRef<HTMLUListElement>(null);
  const stop = items.some((it) => it.id === active) ? active : items[0]?.id;
  const buttons = () => Array.from(list.current?.querySelectorAll<HTMLButtonElement>(".set-nav__item") ?? []);
  const onKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    const all = buttons();
    const to =
      e.key === "ArrowDown" ? Math.min(i + 1, all.length - 1)
      : e.key === "ArrowUp" ? Math.max(i - 1, 0)
      : e.key === "Home" ? 0
      : e.key === "End" ? all.length - 1
      : -1;
    if (to < 0) return;
    e.preventDefault();
    all[to]?.focus();
  };
  return (
    <nav className="set-nav" aria-label="Settings sections">
      <ul ref={list} className="set-nav__list">
        {items.map((it, i) => (
          <li key={it.id} className={cx(it.prototype && !items[i - 1]?.prototype && "set-nav__group")}>
            {it.prototype && !items[i - 1]?.prototype && <span className="set-nav__heading">Prototype</span>}
            <button
              type="button"
              className={cx("set-nav__item", active === it.id && "is-current", it.prototype && "is-prototype")}
              aria-current={active === it.id ? "location" : undefined}
              tabIndex={it.id === stop ? 0 : -1}
              onKeyDown={(e) => onKey(e, i)}
              onClick={() => onGo(it.id)}
            >
              <span className="set-nav__label">{it.label}</span>
              {it.mark === "restart" ? (
                <span className="set-nav__restart">
                  <RotateCw aria-hidden />
                  <span className="sr-only">, {MARK_LABEL.restart}</span>
                </span>
              ) : it.mark ? (
                <span className={cx("set-nav__mark", `is-${it.mark}`)}>
                  <span className="sr-only">, {MARK_LABEL[it.mark]}</span>
                </span>
              ) : null}
            </button>
          </li>
        ))}
      </ul>
    </nav>
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

/** First run: the three things between a fresh install and “Hey Ghost”. The page's focal point. */
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
