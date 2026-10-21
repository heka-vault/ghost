/**
 * Owner: policy. The level scale: four stops on one track, least to most autonomous, so a
 * column of knobs reads as a shape down the page. Each row is a native radio group (Tab
 * enters at the chosen level, arrows move and choose). A filled knob is the operator's own
 * line in the file; a hollow one is the default showing through, and clicking it makes it
 * the operator's own.
 */
import { autonomyLabel } from "@ghost/ui";
import { Code, RotateCcw, TriangleAlert } from "lucide-react";
import { type ReactNode, type RefObject, useEffect, useId } from "react";

import { Badge, IconButton, Tooltip, cx } from "../../ui";
import { LEVEL_ICON, LEVELS } from "./model";

/** `text` with backticked spans as code: “the level `confirm` needs quotes”. */
export function Rich({ text }: { text: string }) {
  const parts = text.split("`");
  return <>{parts.map((p, i) => (i % 2 ? <code key={i} className="policy-code">{p}</code> : p))}</>;
}

/**
 * The column labels, drawn once over the whole list and pinned under the compact bar. Cards
 * scroll under it; it names every row on the page. The knob legend sits at its left end, where
 * the eye looks for a key; the labels themselves are repeated by every radio, so they are
 * hidden from the reader.
 */
export function LevelHeader({ headerRef }: { headerRef?: RefObject<HTMLDivElement | null> }) {
  return (
    <div className="policy-cols" ref={headerRef}>
      <p className="policy-cols__legend">Filled: set by you · hollow: default</p>
      <div className="policy-cols__levels" aria-hidden>
        {LEVELS.map(({ value, icon: Icon }) => (
          <div key={value} className="policy-cols__level">
            <Icon size={14} aria-hidden />
            <span>{autonomyLabel(value)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * Pins the column labels flush under the compact bar and marks them while pinned (the
 * hairline shows only then). The offset is the shell's `--page-sticky-top` on #main: the
 * bottom of its banners plus the compact bar, kept current by the shell.
 */
export function useStuckHeader(ref: RefObject<HTMLDivElement | null>, deps: unknown[]) {
  useEffect(() => {
    const main = document.getElementById("main");
    const el = ref.current;
    if (!main || !el) return;
    let frame = 0;
    const check = () => {
      frame = 0;
      const port = main.getBoundingClientRect().top + main.clientTop;
      const style = getComputedStyle(main);
      const shellTop = parseFloat(style.getPropertyValue("--page-sticky-top"));
      const offset = Number.isFinite(shellTop) ? shellTop : parseFloat(style.getPropertyValue("--page-bar-height")) || 48;
      el.style.setProperty("--policy-cols-top", `${offset}px`);
      const wrap = el.parentElement?.getBoundingClientRect();
      el.classList.toggle("is-stuck", !!wrap && wrap.top < port + offset - 0.5);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(check);
    };
    check();
    main.addEventListener("scroll", schedule, { passive: true });
    const top = main.querySelector(".shell__top");
    const ro = top ? new ResizeObserver(schedule) : null;
    if (top) ro?.observe(top);
    return () => {
      main.removeEventListener("scroll", schedule);
      ro?.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

export interface StopsProps {
  name: string;
  /** The level in force, or "" for none (Everything else on “By risk”). */
  value: string;
  /** The value shows through from a default rather than being set here. */
  inherited?: boolean;
  onChange: (level: string) => void;
  disabled?: boolean;
  labelledBy: string;
  describedBy?: string;
  /** A first stop on the track, in words: “By risk” for Everything else. */
  lead?: { label: string; checked: boolean; onSelect: () => void };
  /**
   * The row has its own line in the file, so Delete or Backspace on the chosen stop removes it
   * (the keyboard path to the reset beside the track). Says what that lands on.
   */
  onReset?: { to: string; run: () => void };
}

export function LevelStops({ name, value, inherited, onChange, disabled, labelledBy, describedBy, lead, onReset }: StopsProps) {
  const resetId = useId();
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className={cx("policy-stops", inherited && "is-inherited", lead && "has-lead", !value && "is-empty", disabled && "is-disabled")}
    >
      {onReset && (
        <span id={resetId} className="sr-only">
          Press Delete to use the default, {onReset.to}.
        </span>
      )}
      <div className="policy-stops__rail">
        {lead && (
          // The first stop on the same track, in words. It is the default of the defaults, so it
          // shows hollow when chosen, like every default on the page.
          <Tooltip content={`${lead.label} · default`} delay={700}>
            <label className={cx("policy-stop policy-stop--lead", lead.checked && "is-on")}>
              <input
                type="radio"
                className="policy-stop__input"
                name={name}
                value=""
                checked={lead.checked}
                disabled={disabled}
                aria-label={`${lead.label}, the default`}
                onChange={lead.onSelect}
              />
              <span className="policy-stop__word" aria-hidden>
                {lead.label}
              </span>
              <span className="policy-stop__knob" aria-hidden />
            </label>
          </Tooltip>
        )}
        <span className="policy-stops__track" aria-hidden />
        {LEVELS.map(({ value: level }) => {
          const on = value === level;
          const pin = inherited && on;
          const label = autonomyLabel(level);
          // Short labels only; the accent ring on hover says the hollow one can be pinned.
          const tip = pin ? `${label} · default` : label;
          return (
            <Tooltip key={level} content={tip} delay={700}>
              <label className={cx("policy-stop", on && "is-on")}>
                <input
                  type="radio"
                  className="policy-stop__input"
                  name={name}
                  value={level}
                  checked={on}
                  disabled={disabled}
                  aria-label={pin ? `${label}, the default` : label}
                  aria-describedby={on && onReset ? resetId : undefined}
                  onChange={() => onChange(level)}
                  // A checked radio never fires change: a click (or Space) on the default
                  // showing through pins it as the operator's own line.
                  onClick={() => {
                    if (pin && !disabled) onChange(level);
                  }}
                  onKeyDown={(e) => {
                    if (disabled) return;
                    if (e.key === " " && pin) {
                      e.preventDefault();
                      onChange(level);
                    } else if ((e.key === "Delete" || e.key === "Backspace") && on && onReset) {
                      e.preventDefault();
                      onReset.run();
                    }
                  }}
                />
                <span className="policy-stop__knob" aria-hidden />
              </label>
            </Tooltip>
          );
        })}
      </div>
    </div>
  );
}

/**
 * Low, medium or high, as the word alone, quiet at rest. High sits a step brighter so it can
 * be found down a column, and turns amber only while the row lets Ghost act alone, the same
 * moment the warning shows.
 */
export function Risk({ risk, alarm }: { risk: string; alarm?: boolean }) {
  return (
    <span className={cx("policy-risk", `policy-risk--${risk}`, alarm && "is-alarm")}>
      {risk ? `${risk[0].toUpperCase()}${risk.slice(1)} risk` : "Risk not given"}
    </span>
  );
}

/** The same mark Settings uses for a value that isn’t saved yet; inline, so rows never grow. */
function Unsaved({ id }: { id?: string }) {
  return (
    <span id={id}>
      <Badge variant="outline" size="sm" className="policy-row__unsaved">
        Unsaved
      </Badge>
    </span>
  );
}

/** “Levels can’t rewrite this one”: said under the row the operator just tried, not in a toast. */
function TomlNote({ id, onOpen }: { id: string; onOpen: () => void }) {
  return (
    <p className="policy-row__warn policy-row__note" id={id} role="status">
      <TriangleAlert size={12} aria-hidden />
      <span>The file sets this one in a way Levels can’t rewrite safely.</span>
      <button type="button" className="policy-link" onClick={onOpen}>
        <Code size={12} aria-hidden />
        Change it in TOML
      </button>
    </p>
  );
}

export interface OperationRowProps {
  source: string;
  /** The key in the file: the operation's name, or `*` for the whole source. */
  name: string;
  /** "" for a row with no risk class (the whole-source row). */
  risk: string;
  title: string;
  description: ReactNode;
  /** The operator's own line for this operation, or undefined. */
  own: string | undefined;
  /** What the row does without its own line; "" when that is each operation's risk. */
  inheritedLevel: string;
  /** Where an inherited level comes from, when that isn't obvious (a source's `*` line); "" otherwise. */
  inheritedFrom: string;
  edited: boolean;
  targets: [string, string][];
  /** “Ghost won’t ask before closing issues.”, shown when a high-risk operation acts alone. */
  riskNote: string;
  disabled?: boolean;
  /** The last edit here couldn't be made from Levels. */
  tomlOnly?: boolean;
  onOpenToml: () => void;
  onChange: (level: string | null) => void;
}

export function OperationRow({
  source,
  name,
  risk,
  title,
  description,
  own,
  inheritedLevel,
  inheritedFrom,
  edited,
  targets,
  riskNote,
  disabled,
  tomlOnly,
  onOpenToml,
  onChange,
}: OperationRowProps) {
  const id = useId();
  const value = own ?? inheritedLevel;
  const alone = risk === "high" && (value === "announce" || value === "quiet");
  const meta = own == null && inheritedFrom;
  // A reset is offered wherever the file has a line to remove, and says what that lands on. It
  // lives in a fixed gutter beside the track, so no row ever changes height when the default
  // moves. It is in the Tab order only where it is visible at rest (an unsaved row); elsewhere
  // it shows on hover and the keyboard reaches the same thing with Delete on the chosen stop.
  const reset = own != null;
  const resetTo = inheritedLevel ? autonomyLabel(inheritedLevel).toLowerCase() : "by risk";
  const resetLabel = own === inheritedLevel ? `Unpin · stays ${resetTo}` : `Use default · ${resetTo}`;
  const described = [meta && `${id}-m`, alone && `${id}-w`, edited && `${id}-u`, tomlOnly && `${id}-n`].filter(Boolean).join(" ") || undefined;
  return (
    <div className={cx("policy-row", edited && "is-edited")}>
      <div className="policy-row__text">
        <div className="policy-row__head">
          <span className="policy-row__title" id={`${id}-t`}>
            {title}
          </span>
          <span className="policy-row__meta">
            {risk && <Risk risk={risk} alarm={alone} />}
            {risk && <span aria-hidden>·</span>}
            <span className="policy-row__key">{name === "*" ? '"*"' : name}</span>
          </span>
          {edited && <Unsaved id={`${id}-u`} />}
        </div>
        <p className="policy-row__description">{description}</p>
        {meta && (
          <p className="policy-row__source" id={`${id}-m`}>
            {inheritedFrom}
          </p>
        )}
        {alone && (
          <p className="policy-row__warn" id={`${id}-w`}>
            <TriangleAlert size={12} aria-hidden />
            {riskNote}
          </p>
        )}
        {targets.map(([target, level]) => {
          const Icon = LEVEL_ICON[level];
          return (
            <p key={target} className="policy-row__target">
              {Icon && <Icon size={12} aria-hidden />}
              <span>
                {autonomyLabel(level)} in <span className="t-mono">{target}</span>
              </span>
            </p>
          );
        })}
        {tomlOnly && <TomlNote id={`${id}-n`} onOpen={onOpenToml} />}
      </div>
      <span className="policy-row__gutter">
        {reset && (
          <IconButton
            size="sm"
            variant="ghost"
            className="policy-row__reset"
            icon={<RotateCcw />}
            label={resetLabel}
            tooltipSide="left"
            disabled={disabled}
            tabIndex={edited ? 0 : -1}
            onClick={() => onChange(null)}
          />
        )}
      </span>
      <LevelStops
        name={`${source}.${name}`}
        value={value}
        inherited={own == null}
        onChange={(lv) => onChange(lv)}
        disabled={disabled}
        labelledBy={`${id}-t`}
        describedBy={described}
        onReset={reset && !disabled ? { to: resetTo, run: () => onChange(null) } : undefined}
      />
    </div>
  );
}

/** "Everything else": the [defaults] level on the same scale, with “By risk” as its own choice. */
export function DefaultRow({
  value,
  edited,
  disabled,
  tomlOnly,
  onOpenToml,
  onChange,
}: {
  value: string;
  edited: boolean;
  disabled?: boolean;
  tomlOnly?: boolean;
  onOpenToml: () => void;
  onChange: (v: string) => void;
}) {
  const id = useId();
  return (
    <div className={cx("policy-row policy-row--default", edited && "is-edited")}>
      <div className="policy-row__text">
        <div className="policy-row__head">
          <span className="policy-row__title" id={`${id}-t`}>
            Everything else
          </span>
          <span className="policy-row__meta">
            <span className="policy-row__key">defaults.autonomy</span>
          </span>
          {edited && <Unsaved />}
        </div>
        <p className="policy-row__description" id={`${id}-d`}>
          {value
            ? `Anything not set below, and anything a new server adds: ${autonomyLabel(value).toLowerCase()}.`
            : "Anything not set below follows its risk. Low risk is done and told; the rest ask first."}
        </p>
        {tomlOnly && <TomlNote id={`${id}-n`} onOpen={onOpenToml} />}
      </div>
      <span className="policy-row__gutter" />
      <LevelStops
        name="defaults"
        value={value}
        onChange={onChange}
        disabled={disabled}
        labelledBy={`${id}-t`}
        describedBy={tomlOnly ? `${id}-d ${id}-n` : `${id}-d`}
        lead={{ label: "By risk", checked: !value, onSelect: () => onChange("") }}
      />
    </div>
  );
}
