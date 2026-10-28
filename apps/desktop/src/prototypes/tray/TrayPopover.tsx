/**
 * Owner: tray. The status popover: what "Show status…" in Ghost's native top-bar menu opens.
 * State, the waiting change, mute, the last change, then Open Ghost and Quit.
 * Every write goes through the same client calls the session uses.
 *
 * Focal point: the waiting change when there is one (the header steps back to one quiet
 * line: a small presence, the state, an icon button), otherwise the header's state and its
 * one action.
 *
 * A change never just vanishes. Confirmed or canceled from here, the card keeps its place
 * for a moment and says what happened where its buttons were; lapsed, it shows Expired.
 * Then it collapses over --duration-base (instantly under reduced motion), so nothing jumps
 * under the pointer while the operator is still looking at it.
 */
import type { PendingProposal } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { AppWindow, Check, CircleCheck, Copy, History, LogOut, Mic, MicOff, Moon } from "lucide-react";
import { type CSSProperties, forwardRef, useCallback, useEffect, useId, useImperativeHandle, useRef, useState } from "react";

import { inTauri, startDaemon } from "../../transport";
import {
  Button,
  IconButton,
  PendingChange,
  Progress,
  Spinner,
  Switch,
  Tooltip,
  VoicePresence,
  cx,
  errorText,
  relativeTime,
  statusLabel,
  useLive,
} from "../../ui";
import { capitalize, doneChange, micLabel, micTitle, shortChange, useLastChange, useMicName } from "./data";
import { actionables, roveKey, useFocusKeeper } from "./focus";
import { type TrayState, lowerFirst } from "./model";
import { tokenMs } from "./motion";

const START_COMMAND = "systemctl --user enable --now ghostd";
/** How long a lapsed change stays on screen, reading "Expired · Ghost left it alone". */
const EXPIRED_HOLD_MS = 3000;
/** How long the result of a Confirm or Cancel stays where the card was. */
const OUTCOME_HOLD_MS = 1500;

type Busy = null | "wake" | "sleep" | "confirm" | "cancel" | "start";

export interface TrayPopoverProps {
  state: TrayState;
  shortcut: string | null;
  onMutedChange: (muted: boolean) => void;
  openHref: string;
  onQuit: () => void;
  /** Focus the first control on open (opened from the keyboard with Down). */
  autoFocus?: boolean;
  /** An error from an action taken in the native menu, which has nowhere to show one. */
  initialError?: string | null;
  id?: string;
}

type Decision = "confirmed" | "canceled";

/** What the card slot shows: the live change, or one that just ended, held for a moment. */
interface Shown {
  proposal: PendingProposal;
  /** live: waiting. deciding: Confirm/Cancel sent, answer not back. expired, or the decision. */
  phase: "live" | "deciding" | "expired" | Decision;
  /** The slot's height when it was decided, so the result keeps the card's footprint. */
  height: number | null;
  collapsing: boolean;
}

/**
 * The change Ghost is waiting on, and for a moment after it ends: a lapsed one stays as
 * Expired for three seconds; one confirmed or canceled here shows its result for 1.5 s.
 * Then the slot collapses over --duration-base and goes. A change decided somewhere else
 * (by voice, in the native menu) simply goes: the Last change row says what happened.
 */
function useShownPending(pending: PendingProposal | null, slot: React.RefObject<HTMLElement | null>) {
  const [held, setHeld] = useState<Shown | null>(null);
  const prev = useRef(pending);
  const deciding = useRef<{ id: string; height: number | null } | null>(null);
  const timers = useRef<number[]>([]);

  const clear = () => {
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current = [];
  };
  useEffect(() => clear, []);

  /** Keep `h` for `ms`, then collapse it and let it go. */
  const holdThenCollapse = useCallback(
    (h: Shown, ms: number) => {
      clear();
      setHeld(h);
      timers.current.push(
        window.setTimeout(() => {
          const height = slot.current?.offsetHeight ?? h.height;
          setHeld((x) => (x && x.proposal.id === h.proposal.id ? { ...x, height, collapsing: true } : x));
          timers.current.push(
            window.setTimeout(
              () => setHeld((x) => (x && x.proposal.id === h.proposal.id ? null : x)),
              tokenMs(slot.current, "--duration-base"),
            ),
          );
        }, ms),
      );
    },
    [slot],
  );

  useEffect(() => {
    const p = prev.current;
    prev.current = pending;
    if (pending) {
      // A new change replaces whatever was held; the same one stays as decided.
      setHeld((x) => (x && x.proposal.id !== pending.id ? (clear(), null) : x));
      return;
    }
    if (!p) return;
    if (deciding.current?.id === p.id) {
      setHeld((x) => x ?? { proposal: p, phase: "deciding", height: deciding.current?.height ?? null, collapsing: false });
    } else if (p.expires_at && Date.parse(p.expires_at) <= Date.now() + 1000) {
      holdThenCollapse({ proposal: p, phase: "expired", height: null, collapsing: false }, EXPIRED_HOLD_MS);
    }
  }, [pending, holdThenCollapse]);

  /** Confirm or Cancel was pressed on `p`: remember its footprint. */
  const begin = useCallback(
    (p: PendingProposal) => {
      deciding.current = { id: p.id, height: slot.current?.offsetHeight ?? null };
    },
    [slot],
  );
  /** The daemon answered: show the result in the card's place, or drop the hold on failure. */
  const settle = useCallback(
    (p: PendingProposal, result: Decision | null) => {
      const height = deciding.current?.id === p.id ? deciding.current.height : null;
      deciding.current = null;
      if (result) holdThenCollapse({ proposal: p, phase: result, height, collapsing: false }, OUTCOME_HOLD_MS);
      else setHeld((x) => (x?.phase === "deciding" ? null : x));
    },
    [holdThenCollapse],
  );

  const shown: Shown | null =
    held && (!pending || held.proposal.id === pending.id)
      ? held
      : pending
        ? { proposal: pending, phase: "live", height: null, collapsing: false }
        : null;
  return { shown, begin, settle };
}

export const TrayPopover = forwardRef<HTMLElement, TrayPopoverProps>(function TrayPopover(
  { state, shortcut, onMutedChange, openHref, onQuit, autoFocus, initialError, id },
  ref,
) {
  const { client, status, pending } = useGhost();
  const { connection, online, loadingModels, setupNeeded, setupHref } = useLive();
  const { entry: last } = useLastChange();
  const mic = useMicName();
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(initialError ?? null);
  const [said, setSaid] = useState("");
  const pendingSlot = useRef<HTMLDivElement>(null);
  const { shown, begin, settle } = useShownPending(pending, pendingSlot);
  const quitMeta = useId();
  const micDesc = useId();

  const root = useRef<HTMLElement>(null);
  const actionSlot = useRef<HTMLDivElement>(null);
  useImperativeHandle(ref, () => root.current as HTMLElement);

  // Where focus goes when the control that had it disappears: the header's action (Go to
  // sleep replaces Wake, and the other way round), else the first control in the menu.
  const slotButton = () => actionSlot.current?.querySelector<HTMLElement>("button:not([disabled]), a[href]") ?? null;
  const focus = useFocusKeeper(root, () => slotButton() ?? actionables(root.current)[0] ?? root.current);

  useEffect(() => {
    if (autoFocus) actionables(root.current)[0]?.focus();
  }, [autoFocus]);

  // One short spoken-style confirmation for screen readers after Confirm and Cancel.
  useEffect(() => {
    if (!said) return;
    const t = window.setTimeout(() => setSaid(""), 2500);
    return () => window.clearTimeout(t);
  }, [said]);

  const run = async (kind: Exclude<Busy, null>, f: () => Promise<unknown>, onError = setError, done?: string): Promise<boolean> => {
    setBusy(kind);
    onError(null);
    try {
      await f();
      if (done) setSaid(done);
      return true;
    } catch (e) {
      onError(errorText(e));
      return false;
    } finally {
      setBusy(null);
    }
  };

  /** Confirm or Cancel: the same client calls as ever, with the result held in the card's place. */
  const decide = async (p: PendingProposal, d: Decision) => {
    focus.request(slotButton);
    begin(p);
    const ok =
      d === "confirmed"
        ? await run("confirm", () => client.confirm(), setError, `Done. ${doneChange(p.summary, p.target)}.`)
        : await run("cancel", () => client.cancel(), setError, "Canceled. Ghost left it alone.");
    settle(p, ok ? d : null);
  };

  const offline = connection === "offline" || connection === "connecting";
  const passive = online && !state.inSession && !loadingModels;
  const quiet = !!shown;

  // One action beside the state, in the same place every time: Wake while passive (the
  // popover's one primary), Go to sleep in a session, Start Ghost when it is stopped.
  let action = null;
  if (connection === "offline" && inTauri) {
    action = (
      <Button variant="primary" size="sm" loading={busy === "start"} onClick={() => run("start", startDaemon)}>
        Start Ghost
      </Button>
    );
  } else if (state.inSession && quiet) {
    // A change is waiting: the way out stays, one icon wide, so the card leads.
    action = (
      <IconButton
        label="Go to sleep"
        icon={<Moon />}
        variant="ghost"
        tooltipSide="bottom"
        disabled={!online}
        loading={busy === "sleep"}
        onClick={() => {
          focus.request(slotButton);
          void run("sleep", () => client.sleep());
        }}
      />
    );
  } else if (state.inSession) {
    // Also during a reconnect, disabled in place, as the HUD and the native menu keep theirs.
    action = (
      <Button
        size="sm"
        leadingIcon={<Moon />}
        disabled={!online}
        loading={busy === "sleep"}
        onClick={() => {
          focus.request(slotButton);
          void run("sleep", () => client.sleep());
        }}
      >
        Go to sleep
      </Button>
    );
  } else if (passive && setupNeeded) {
    // Waking is pointless without a model password: the one action is the way to fix that.
    action = (
      <Button variant="primary" size="sm" href={setupHref}>
        Finish setup
      </Button>
    );
  } else if (passive && !state.muted) {
    const wake = (
      <Button
        variant="primary"
        size="sm"
        disabled={!status?.models_ready}
        loading={busy === "wake"}
        onClick={() => {
          focus.request(slotButton);
          void run("wake", () => client.wake());
        }}
      >
        Wake
      </Button>
    );
    action = shortcut ? <Tooltip content="Wake Ghost" shortcut={shortcut}>{wake}</Tooltip> : wake;
  }

  // One short phrase ("Approved app 208"); the gate's full words are its tooltip and what a
  // screen reader hears, never a third visible line that restates the lead.
  const lastDetail = last ? last.summary : "";
  const lastLead = !last
    ? ""
    : last.status === "executed"
      ? doneChange(last.summary, last.target)
      : capitalize(shortChange(last.summary, last.target));

  return (
    <section
      ref={root}
      id={id}
      className="tray-pop"
      role="dialog"
      aria-label="Ghost"
      tabIndex={-1}
      data-state={state.presence}
      onKeyDown={(e) => roveKey(e, root.current)}
    >
      <header className={cx("tray-pop__head", quiet && "is-quiet")}>
        <VoicePresence phase={state.presence} size={48} label="" />
        <div className="tray-pop__state">
          <p className={cx("tray-pop__title", connection === "reconnecting" && "t-warn")} aria-live="polite">
            {state.title}
          </p>
          {quiet && (
            <span className="tray-pop__sep" aria-hidden>
              ·
            </span>
          )}
          {/* After the separator the sub is mid-sentence, so it reads "Listening · on the main
              line", as the native status line does; on its own line it keeps its capital. */}
          <p className="tray-pop__sub">{quiet ? lowerFirst(state.sub) : state.sub}</p>
        </div>
        {action && (
          <div ref={actionSlot} className="tray-pop__action">
            {action}
          </div>
        )}
      </header>
      {/* Starting: the bar runs the header's full padding width, left edge of the presence to
          the right padding, so its ends line up with the rows and separators below. */}
      {online && loadingModels && <Progress className="tray-pop__progress" label="Loading voice models" />}

      <p className="sr-only" role="status">
        {said}
      </p>

      {connection === "offline" && !inTauri && <StartCommand />}

      {error && (
        <p className="tray-pop__error" role="alert">
          {error}
        </p>
      )}

      {shown && (
        <div
          ref={pendingSlot}
          className={cx("tray-pop__pending", shown.height != null && "is-held", shown.collapsing && "is-collapsing")}
          style={shown.height != null ? ({ "--tray-h": `${shown.height}px` } as CSSProperties) : undefined}
        >
          {shown.phase === "confirmed" || shown.phase === "canceled" ? (
            <Outcome proposal={shown.proposal} decision={shown.phase} />
          ) : (
            <PendingChange
              compact
              summary={shown.proposal.summary}
              rationale={shown.proposal.rationale}
              operation={`${shown.proposal.source}.${shown.proposal.operation}`}
              expiresAt={shown.proposal.expires_at}
              busy={busy === "confirm" ? "confirm" : busy === "cancel" ? "cancel" : shown.phase === "deciding"}
              disabled={!online || shown.phase === "expired"}
              onConfirm={() => void decide(shown.proposal, "confirmed")}
              onCancel={() => void decide(shown.proposal, "canceled")}
            />
          )}
        </div>
      )}

      {!offline && (
        <div className="tray-pop__group tray-pop__rows">
          <div className="tray-row">
            <span className={cx("tray-row__icon", state.muted && "t-warn")} aria-hidden>
              {state.muted ? <MicOff /> : <Mic />}
            </span>
            <span className="tray-row__text">
              <span className="tray-row__title">Mute microphone</span>
              {/* One clause: the device, as the operator knows it. What stands behind a named
                  device (ghost.mic is the echo-canceled source) is the tooltip, and so is a raw
                  id when that is all we have. */}
              <span id={micDesc} className="tray-row__desc" title={micTitle(mic)}>
                {micLabel(mic)}
              </span>
            </span>
            {/* No mute API yet: the control exists only in the mock layer. The mark sits beside
                the switch, out of the sentence, so the description never clips. */}
            <span className="tray-row__tag" aria-hidden>
              Prototype
            </span>
            <Switch
              checked={state.muted}
              onChange={onMutedChange}
              disabled={!online}
              aria-label="Mute microphone (prototype)"
              aria-describedby={micDesc}
            />
          </div>
        </div>
      )}

      {/* Only when there is something to show: the empty explanation lives on Audit. The
          block starts on the same icon column as the rows around it, so one text edge runs
          through the whole menu. */}
      {last && (
        <div className="tray-pop__group tray-pop__last">
          <span className="tray-pop__last-icon" aria-hidden>
            <History />
          </span>
          <div className="tray-pop__last-text">
            <p className="tray-pop__label">
              Last change · {relativeTime(last.created_at)}
              {last.status !== "executed" && <> · {statusLabel(last.status).toLowerCase()}</>}
            </p>
            <p className={cx("tray-pop__done", last.status !== "executed" && "is-settled")} title={lastDetail}>
              {lastLead}
              {lastDetail !== lastLead && (
                <span className="sr-only" data-user-content>
                  {" "}
                  ({lastDetail})
                </span>
              )}
            </p>
          </div>
        </div>
      )}

      <div className="tray-pop__group tray-pop__menu">
        <a className="tray-item" href={openHref}>
          <AppWindow aria-hidden />
          <span className="tray-item__label">Open Ghost</span>
        </a>
        <button
          type="button"
          className="tray-item"
          aria-label="Quit"
          aria-describedby={online ? quitMeta : undefined}
          onClick={onQuit}
        >
          <LogOut aria-hidden />
          <span className="tray-item__label">Quit</span>
          {online && (
            <span id={quitMeta} className="tray-item__meta">
              Ghost keeps running
            </span>
          )}
        </button>
      </div>
    </section>
  );
});

/**
 * Where the card was, after Confirm or Cancel: what it was, and one line saying what
 * happened in the place its buttons were. No button here: a second click on Confirm must
 * land on nothing.
 */
function Outcome({ proposal, decision }: { proposal: PendingProposal; decision: Decision }) {
  const confirmed = decision === "confirmed";
  return (
    <div className={cx("tray-outcome", confirmed ? "is-confirmed" : "is-canceled")}>
      <p className="tray-outcome__state">{confirmed ? "Confirmed" : "Canceled"}</p>
      <p className="tray-outcome__summary" data-user-content>
        {proposal.summary}
      </p>
      <p className="tray-outcome__operation">{`${proposal.source}.${proposal.operation}`}</p>
      {proposal.rationale && (
        <p className="tray-outcome__rationale" data-user-content>
          {proposal.rationale}
        </p>
      )}
      <p className="tray-outcome__result">
        {confirmed ? (
          <>
            <CircleCheck aria-hidden className="tray-outcome__icon" />
            {doneChange(proposal.summary, proposal.target)}
          </>
        ) : (
          "Ghost left it alone"
        )}
      </p>
    </div>
  );
}

/** Browser builds can't start the service; show the command, as the Offline page does. */
function StartCommand() {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(START_COMMAND);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable: the command is selectable */
    }
  };
  return (
    <div className="tray-pop__slot">
      <div className="tray-pop__command">
        <code>{START_COMMAND}</code>
        <IconButton size="sm" label={copied ? "Copied" : "Copy command"} icon={copied ? <Check /> : <Copy />} onClick={copy} />
      </div>
      <p className="tray-pop__hint tray-pop__retry">
        <Spinner size={12} /> Checking again every few seconds
      </p>
    </div>
  );
}

