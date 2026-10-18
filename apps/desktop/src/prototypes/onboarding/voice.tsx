/** Owner: onboarding. The voice steps: microphone and echo cancellation, voice models, the service, a first “Hey Ghost”. */
import { Check, CircleCheck, Copy, Download, HardDrive, MicOff } from "lucide-react";
import { useGhost } from "@ghost/ui";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { inTauri, startDaemon } from "../../transport";
import {
  Badge,
  Button,
  Card,
  cx,
  Field,
  IconButton,
  ListRow,
  Notice,
  Progress,
  quoted,
  Select,
  Skeleton,
  Spinner,
  errorText,
  useLive,
} from "../../ui";
import type { Wizard } from "./index";
import { HeroPresence, LevelMeter, PrototypeTag, Row, StepFrame } from "./frame";
import { download, formatMb, phraseCase, splitList, tildify, timeLeft, useDownload } from "./state";
import { NeedsGhost, useSavesWhenBack } from "./steps";

/* -------------------------------------------------------------------------- microphone */

export function MicrophoneStep({ w }: { w: Wizard }) {
  const { online } = useLive();
  const d = w.devices;
  const saved = w.settings?.values.GHOST_INPUT_DEVICE ?? "";
  const [device, setDevice] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ tone: "ok" | "danger"; title: string; text: string } | null>(null);

  // Once ghost.mic exists and nothing is chosen yet, suggest it (saved on Continue, not before).
  const value = device ?? (saved || (d?.echo_cancel_source_present ? "ghost.mic" : ""));
  const inputs = (d?.devices ?? []).filter((x) => x.max_input_channels > 0 && x.name !== "ghost.mic");
  const options = [
    { value: "", label: "System default" },
    ...(d?.echo_cancel_source_present || value === "ghost.mic" ? [{ value: "ghost.mic", label: "ghost.mic · echo canceled" }] : []),
    ...inputs.map((x) => ({ value: x.name, label: x.name })),
    ...(value && value !== "ghost.mic" && !inputs.some((x) => x.name === value) ? [{ value, label: value }] : []),
  ];

  const install = async () => {
    setInstalling(true);
    setResult(null);
    try {
      const r = await w.client.setupEchoCancel(false);
      // Success shows in place: the row turns Installed and ghost.mic is picked below.
      setResult(
        r.ok
          ? null
          : { tone: "danger", title: "Couldn’t install echo cancellation", text: r.detail ?? "The installer stopped without saying why. Try again, or run ghostd setup echo-cancel in a terminal." },
      );
      await w.reload();
    } catch (e) {
      setResult({ tone: "danger", title: "Couldn’t install echo cancellation", text: errorText(e) });
    } finally {
      setInstalling(false);
    }
  };

  const submit = async () => {
    if (value === saved) return w.next();
    setBusy(true);
    setResult(null);
    try {
      await w.save({ values: { GHOST_INPUT_DEVICE: value || null } });
      w.next();
    } catch (e) {
      setResult({ tone: "danger", title: "Couldn’t save", text: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const active = !!d?.echo_cancel_active;
  const mic = <span className="t-mono">ghost.mic</span>;
  const help = !d?.echo_cancel_source_present ? (
    <>Install echo cancellation first to get {mic}, the one Ghost should use.</>
  ) : value === "ghost.mic" ? (
    "The right one: your microphone, minus Ghost’s own voice."
  ) : (
    <>Pick {mic}, so Ghost doesn’t hear itself.</>
  );
  const footNote = useSavesWhenBack(value !== saved);

  return (
    <StepFrame
      id="microphone"
      title="Set up the microphone"
      description={
        <>
          Ghost needs to hear you over its own voice and your music. Echo cancellation adds a cleaned-up microphone called {mic} for it to listen on.
        </>
      }
      primary={{ label: "Continue", onClick: submit, loading: busy, disabled: (w.loading && !d) || (!online && value !== saved) }}
      onBack={w.back}
      onSkip={w.done.microphone ? undefined : w.skip}
      footNote={footNote}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="set up audio" />
          {result && (
            <Notice tone={result.tone} title={result.title} onDismiss={() => setResult(null)}>
              {result.text}
            </Notice>
          )}
        </>
      }
    >
      {!d ? (
        <Card list aria-hidden>
          <Row>
            <Skeleton width={180} height={14} />
            <Skeleton width={320} height={12} />
          </Row>
          <Row>
            <Skeleton width={120} height={12} />
            <Skeleton height={32} radius="var(--radius-md)" />
          </Row>
        </Card>
      ) : (
        <>
          <Card list>
            <ListRow
              title="Echo cancellation"
              description={active ? "Running. PipeWire filters Ghost’s voice out of your microphone." : <>One click. It adds {mic} and leaves PipeWire running.</>}
              trailing={
                active ? (
                  <Badge tone="ok" dot>
                    Installed
                  </Badge>
                ) : (
                  <Button size="sm" loading={installing} disabled={!online} onClick={install}>
                    Install
                  </Button>
                )
              }
            />
            <Row>
              <Field label="Microphone" detail="GHOST_INPUT_DEVICE" help={help}>
                <Select value={value} onChange={(e) => setDevice(e.target.value)} options={options} />
              </Field>
            </Row>
            {w.fixture && (
              <MicCheck
                levels={w.fixture.micLevels}
                quiet={w.demo === "quiet"}
                device={options.find((o) => o.value === value)?.label.replace(" · echo canceled", "") ?? "System default"}
                ghostMic={value === "ghost.mic"}
              />
            )}
          </Card>
          {!w.fixture && <p className="onb-help onb-after-card">You’ll check it in the last step, by saying “Hey Ghost”.</p>}
        </>
      )}
    </StepFrame>
  );
}

/** How long the meter may stay flat before the step says nothing is coming through. */
const SILENT_AFTER_MS = 5_000;

/** The live level, isolated so only this row re-renders ten times a second. */
function MicCheck({ levels, quiet, device, ghostMic }: { levels: number[]; quiet: boolean; device: string; ghostMic: boolean }) {
  const [i, setI] = useState(0);
  const [heard, setHeard] = useState(false);
  // ?state=quiet designs the problem line, so it starts there; live, it takes five flat seconds.
  const [silent, setSilent] = useState(quiet);
  const lastSound = useRef(Date.now());
  useEffect(() => {
    const t = setInterval(() => setI((n) => n + 1), 100);
    return () => clearInterval(t);
  }, []);
  const level = quiet ? 0.02 : levels[i % levels.length];
  useEffect(() => {
    if (level > 0.1) lastSound.current = Date.now();
    if (level > 0.35) setHeard(true);
    if (!heard && Date.now() - lastSound.current > SILENT_AFTER_MS) setSilent(true);
    else if (level > 0.1) setSilent(false);
  }, [level, heard]);
  // A new microphone starts the check over.
  useEffect(() => {
    lastSound.current = Date.now();
    setHeard(false);
    setSilent(quiet);
  }, [device, quiet]);

  const problem = silent && !heard;
  return (
    <Row className="onb-mic">
      <div className="onb-mic__head">
        <span className="onb-mic__label">Say something</span>
        <PrototypeTag />
      </div>
      <LevelMeter level={level} label={`Input level on ${device}`} />
      {/* Green only once ghost.mic is in use: on any other microphone Ghost still hears itself, so hearing you isn't the finish line. */}
      <p className={cx("onb-mic__status", heard && (ghostMic ? "is-ok" : "is-heard"), problem && "is-problem")} aria-live="polite">
        {heard && ghostMic ? (
          <>
            <CircleCheck size={14} aria-hidden /> Ghost hears you clearly
          </>
        ) : heard ? (
          <span>
            Sound’s coming through. Ghost will hear itself too until it listens on <span className="t-mono">ghost.mic</span>.
          </span>
        ) : problem ? (
          <>
            <MicOff size={14} aria-hidden /> Nothing’s coming through on {device}. Check it isn’t muted, or pick another microphone.
          </>
        ) : (
          "Talk at your normal volume. The bar should reach the middle."
        )}
      </p>
    </Row>
  );
}

/* ------------------------------------------------------------------------------ models */

export function ModelsStep({ w }: { w: Wizard }) {
  const dl = useDownload();
  const f = w.fixture;

  if (!f) return <ModelsStepLive w={w} />;

  const total = f.models.reduce((a, m) => a + m.sizeMb, 0);
  const got = f.models.reduce((a, m) => a + Math.min(m.sizeMb, dl.got[m.id] ?? 0), 0);
  const current = f.models.find((m) => (dl.got[m.id] ?? 0) < m.sizeMb);
  const failed = dl.phase === "failed";
  const paused = dl.phase === "idle" && got > 0;

  const primary =
    dl.phase === "done" || dl.phase === "downloading"
      ? { label: "Continue", onClick: w.next }
      : failed
        ? { label: "Try again", onClick: () => download.start(), icon: <Download /> }
        : { label: got > 0 ? `Resume · ${formatMb(total - got)} left` : `Download ${formatMb(total)}`, onClick: () => download.start(), icon: <Download /> };

  // One measure for the whole download: the header line and the bar under it. Rows say only where each model is.
  return (
    <StepFrame
      id="models"
      title="Get the voice models"
      description={
        <>
          Ghost speaks and listens with two models that run on this computer: about {formatMb(total)}, downloaded once to{" "}
          <span className="t-mono">~/.cache/ghost</span>. After that, speech never needs the internet.
        </>
      }
      primary={primary}
      secondary={
        dl.phase === "downloading" ? (
          <Button variant="ghost" onClick={() => download.cancel()}>
            Pause
          </Button>
        ) : undefined
      }
      onBack={w.back}
      onSkip={dl.phase === "idle" || failed ? w.skip : undefined}
      autoFocus={w.focusOnOpen}
      notices={
        failed ? (
          <Notice tone="danger" title="Download stopped">
            Check the internet connection and try again. It picks up where it left off.
          </Notice>
        ) : undefined
      }
    >
      <div className="onb-dl">
        <div className="onb-dl__head">
          <p className="onb-dl__total t-num" aria-live="off">
            {dl.phase === "done" ? (
              <>All {formatMb(total)} on this computer</>
            ) : failed || got > 0 ? (
              <>
                {formatMb(got)} <span className="t-3">of {formatMb(total)}</span>
                {!failed && <span className="t-3"> · {dl.phase === "downloading" ? timeLeft(total - got, dl.rate) : "paused"}</span>}
              </>
            ) : (
              <>{formatMb(total)} to download</>
            )}
          </p>
          <PrototypeTag />
        </div>
        {dl.phase !== "done" && (got > 0 || dl.phase === "downloading") && (
          <Progress className="onb-dl__bar" tone={failed ? "neutral" : "accent"} value={(got / total) * 100} label="Voice models downloaded" />
        )}
        {/* One announcement per phase, not per tick. A stop is announced by the danger Notice (role alert), nowhere else. */}
        <p className="sr-only" aria-live="polite">
          {dl.phase === "downloading" ? "Downloading the voice models." : dl.phase === "done" ? "The voice models are downloaded." : paused ? "Paused." : ""}
        </p>
        <Card list className="onb-dl__list">
          {f.models.map((m) => {
            const have = Math.min(m.sizeMb, dl.got[m.id] ?? 0);
            const doneOne = have >= m.sizeMb;
            const isCurrent = current?.id === m.id && (dl.phase === "downloading" || failed || paused);
            return (
              <Row key={m.id} className="onb-model">
                <div className="onb-model__line">
                  <span className="onb-tile onb-tile--sm" aria-hidden>
                    <HardDrive />
                  </span>
                  <div className="onb-model__text">
                    <p className="onb-model__title">{m.title}</p>
                    <p className="t-small t-3">
                      <span className="t-mono">{m.detail}</span> · {formatMb(m.sizeMb)}
                    </p>
                  </div>
                  <div className="onb-model__state">
                    {doneOne ? (
                      <Badge tone="ok" dot>
                        On this computer
                      </Badge>
                    ) : isCurrent && failed ? (
                      <span className="t-small t-3 t-num">
                        {formatMb(have)} of {formatMb(m.sizeMb)}
                      </span>
                    ) : isCurrent && paused ? (
                      <span className="t-small t-2">Paused</span>
                    ) : isCurrent ? (
                      <span className="onb-model__working t-small t-2">
                        <Spinner size={12} label="" /> Downloading
                      </span>
                    ) : dl.phase === "downloading" ? (
                      <span className="t-small t-3">Next</span>
                    ) : null}
                  </div>
                </div>
              </Row>
            );
          })}
        </Card>
        {dl.phase === "downloading" && <p className="onb-help">You can go on with setup while this finishes.</p>}
      </div>
    </StepFrame>
  );
}

/** Against the real daemon: GET /setup/models says what's there; the download itself has no API yet. */
function ModelsStepLive({ w }: { w: Wizard }) {
  const m = w.models;
  const rows: { title: string; detail: ReactNode; ok: boolean }[] = [
    { title: "Ghost’s voice", detail: "Kokoro", ok: !!m?.kokoro_present },
    { title: "Ghost’s ears", detail: m?.stt_model ?? "Parakeet", ok: !!m?.ready },
  ];
  return (
    <StepFrame
      id="models"
      title="Get the voice models"
      description={
        <>
          Ghost speaks and listens with two models that run on this computer: about 1.6 GB, downloaded once to{" "}
          <span className="t-mono">{tildify(m?.cache_dir ?? "~/.cache/ghost")}</span>. After that, speech never needs the internet.
        </>
      }
      primary={{ label: "Continue", onClick: w.next }}
      onBack={w.back}
      autoFocus={w.focusOnOpen}
      notices={
        m && !m.ready ? (
          <Notice tone="info" title="Ghost fetches these when it starts">
            Start Ghost in the next step and it downloads them on its own. To fetch them now, run <span className="t-mono">ghostd setup</span>.
          </Notice>
        ) : (
          <NeedsGhost w={w} what="check the models" />
        )
      }
    >
      <Card list>
        {rows.map((r) => (
          <ListRow
            key={r.title}
            leading={
              <span className="onb-tile onb-tile--sm" aria-hidden>
                <HardDrive />
              </span>
            }
            title={r.title}
            description={<span className="t-mono">{r.detail}</span>}
            trailing={
              !m ? (
                <Skeleton width={96} height={20} />
              ) : r.ok ? (
                <Badge tone="ok" dot>
                  On this computer
                </Badge>
              ) : (
                <Badge>Not downloaded</Badge>
              )
            }
          />
        ))}
      </Card>
    </StepFrame>
  );
}

/* ----------------------------------------------------------------------------- service */

const START = "systemctl --user enable --now ghostd";
const RESTART = "systemctl --user restart ghostd";

type Check = "done" | "working" | "waiting" | "problem";

function CheckRow({ state, n, title, description, trailing }: { state: Check; n: number; title: string; description: ReactNode; trailing?: ReactNode }) {
  return (
    <li className={cx("onb-check", `is-${state}`)}>
      <span className="onb-check__mark" aria-hidden>
        {state === "done" ? <Check size={12} strokeWidth={2.5} /> : state === "working" ? <Spinner size={14} label="" /> : <span className="t-num">{n}</span>}
      </span>
      <div className="onb-check__text">
        <p className="onb-check__title">
          {title}
          <span className="sr-only">{state === "done" ? ", done" : state === "working" ? ", in progress" : state === "problem" ? ", needs you" : ", waiting"}</span>
        </p>
        <div className="onb-check__description">{description}</div>
      </div>
      {trailing && <div className="onb-check__trailing">{trailing}</div>}
    </li>
  );
}

export function CommandLine({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable: the command is selectable */
    }
  };
  return (
    <div className="onb-command">
      <code>{command}</code>
      <IconButton size="sm" label={copied ? "Copied" : "Copy command"} icon={copied ? <Check /> : <Copy />} onClick={copy} />
    </div>
  );
}

export function ServiceStep({ w }: { w: Wizard }) {
  const live = useLive();
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const online = live.online;
  // One truth with the models step: loaded only when Ghost says so and the models are on disk.
  const modelsLoaded = w.voiceReady;
  const needModels = online && !w.done.models;
  const phrase = phraseCase(splitList(w.settings?.values.GHOST_WAKE_PHRASES)[0] ?? "hey ghost");

  const start = async () => {
    setStarting(true);
    setError(null);
    try {
      await startDaemon();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(false);
    }
  };

  // Reconnecting keeps the last known answer, as the rail does: it was running, this window is
  // waiting for it to answer again. Nothing is marked done or broken on a dropped connection.
  const reconnecting = live.connection === "reconnecting";
  const running: Check = online ? "done" : live.connection === "connecting" || reconnecting || starting ? "working" : "problem";
  const canStart = !online && inTauri;
  // In the browser with Ghost not running there is nothing to click here: the honest verb is skip.
  const offlineHere = live.connection === "offline" && !inTauri;

  return (
    <StepFrame
      id="service"
      title="Start Ghost"
      description="Ghost runs quietly in the background as your own user service, and starts again whenever you log in. This window is only a view onto it."
      primary={canStart ? { label: "Start Ghost", onClick: start, loading: starting } : offlineHere ? undefined : { label: "Continue", onClick: w.next }}
      onBack={w.back}
      onSkip={canStart || offlineHere ? w.skip : undefined}
      skipLabel={offlineHere ? "Continue without Ghost" : undefined}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          {error && (
            <Notice tone="danger" title="Ghost didn’t start">
              {error} Run <span className="t-mono">systemctl --user status ghostd</span> to see why.
            </Notice>
          )}
          {online && w.saved.restartNeeded && (
            <Notice
              tone="info"
              title="Restart Ghost to apply"
              action={
                <Button size="sm" variant="ghost" onClick={() => w.setSaved({ restartNeeded: false })}>
                  Done
                </Button>
              }
            >
              Some of what you saved applies only after a restart.
              <CommandLine command={RESTART} />
            </Notice>
          )}
        </>
      }
    >
      <ol className="onb-checks">
        <CheckRow
          state={running}
          n={1}
          title={online ? "Ghost is running" : reconnecting ? "Reconnecting to Ghost" : "Start the background service"}
          description={
            online ? (
              <>
                A user service on this computer, <span className="t-mono">ghostd</span>.
              </>
            ) : reconnecting ? (
              "It was running a moment ago. This window is waiting for it to answer again."
            ) : inTauri ? (
              "Not running yet. Start it here, or it starts by itself next time you log in."
            ) : (
              <>
                Not running yet. Start it from a terminal:
                <CommandLine command={START} />
              </>
            )
          }
        />
        <CheckRow
          state={modelsLoaded ? "done" : online && !needModels ? "working" : "waiting"}
          n={2}
          title={modelsLoaded ? "Voice models loaded" : "Load the voice models"}
          description={
            modelsLoaded
              ? "Ready to speak and listen."
              : needModels
                ? "They aren’t on this computer yet. Get them first; Ghost loads them right after."
                : online
                  ? "Loading. The first time takes about a minute."
                  : reconnecting
                    ? "Once Ghost is back."
                    : "Once Ghost is running."
          }
          trailing={
            needModels ? (
              <Button size="sm" variant="secondary" onClick={() => w.go("models")}>
                Get the voice models
              </Button>
            ) : undefined
          }
        />
        <CheckRow
          state={modelsLoaded ? "done" : "waiting"}
          n={3}
          title={modelsLoaded ? `Listening for ${quoted(phrase)}` : `Listen for ${quoted(phrase)}`}
          description="Only for its name, with models on this computer. Nothing leaves until it hears it."
        />
      </ol>
    </StepFrame>
  );
}

/* --------------------------------------------------------------------------------- try */

/** How long Ghost may stay quiet, ready and listening, before the step offers help. */
const NOT_HEARD_AFTER_MS = 15_000;
/** A wake within this long of our own click is the click's. */
const CLICK_WINDOW_MS = 1_500;
/** How long to watch a new session for a typed first turn before calling the wake a voice wake. */
const SETTLE_MS = 700;

/**
 * Watches Ghost's phase while the step is open and says how each wake happened. Only a change
 * from passive to awake seen here counts: a session that was already running when the step
 * opened is not a wake. A wake right after our own click is a click. A session whose first
 * user turn arrives typed (the session composer) is not a voice wake either.
 */
function useWakeWatch(awake: boolean, onWake: (how: "voice" | "click") => void) {
  const { status, turns } = useGhost();
  const sessionId = status?.session_id ?? null;
  const clickAt = useRef(0);
  const prev = useRef(awake);
  const [alreadyAwake, setAlreadyAwake] = useState(awake);
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  const report = useRef(onWake);
  report.current = onWake;

  useEffect(() => {
    const was = prev.current;
    prev.current = awake;
    if (was && !awake) setAlreadyAwake(false);
    if (was || !awake) return;
    if (Date.now() - clickAt.current < CLICK_WINDOW_MS) return report.current("click");
    const session = sessionId;
    const t = setTimeout(() => {
      const typed = turnsRef.current.some((x) => x.role === "user" && x.via !== "voice" && (!session || x.session_id === session));
      if (!typed) report.current("voice");
    }, SETTLE_MS);
    return () => clearTimeout(t);
    // Only the phase edge matters; the session id is read at that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awake]);

  return { alreadyAwake: alreadyAwake && awake, markClick: () => (clickAt.current = Date.now()) };
}

export function TryStep({ w }: { w: Wizard }) {
  const live = useLive();
  const [waking, setWaking] = useState(false);
  const [sleeping, setSleeping] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<{ title: string; text: string } | null>(null);
  const phrases = splitList(w.settings?.values.GHOST_WAKE_PHRASES);
  const list = phrases.length ? phrases : (w.fixture?.suggestedWakePhrases.slice(0, 1) ?? ["hey ghost"]);
  const phrase = phraseCase(list[0]);
  const others = list.slice(1).map(phraseCase);
  const device = w.settings?.values.GHOST_INPUT_DEVICE;
  const awake = live.online && ["listening", "thinking", "speaking"].includes(live.phase);
  const heard = w.saved.heard ?? (w.fixture && w.demo === "heard" ? "voice" : null);
  const ready = w.voiceReady;

  // The real signal: Ghost's own phase leaves passive while this step is open. A click is only a click.
  const { alreadyAwake, markClick } = useWakeWatch(awake, (how) => {
    if (w.saved.heard !== "voice" && w.saved.heard !== how) w.setSaved({ heard: how });
  });

  // Ready, listening and nothing heard for a while: offer help instead of waiting forever.
  const [quiet, setQuiet] = useState(w.demo === "nothear");
  const waiting = ready && !awake && heard !== "voice";
  useEffect(() => {
    if (!waiting || w.demo === "nothear") return;
    setQuiet(false);
    const t = setTimeout(() => setQuiet(true), NOT_HEARD_AFTER_MS);
    return () => clearTimeout(t);
  }, [waiting, w.demo]);

  const wake = async () => {
    setWaking(true);
    setError(null);
    markClick();
    try {
      await w.client.wake();
    } catch (e) {
      setError({ title: "Ghost didn’t wake", text: errorText(e) });
    } finally {
      setWaking(false);
    }
  };
  const sleep = async () => {
    setSleeping(true);
    setError(null);
    try {
      await w.client.sleep();
    } catch (e) {
      setError({ title: "Ghost didn’t go to sleep", text: errorText(e) });
    } finally {
      setSleeping(false);
    }
  };
  const start = async () => {
    setStarting(true);
    setError(null);
    try {
      await startDaemon();
    } catch (e) {
      setError({ title: "Ghost didn’t start", text: `${errorText(e)} Run systemctl --user status ghostd to see why.` });
    } finally {
      setStarting(false);
    }
  };

  const mic = device ? <span className="t-mono">{device}</span> : "your default microphone";

  // One focal line per state; the “Say …” phrase only when Ghost can actually hear it.
  let title: ReactNode;
  let body: ReactNode = null;
  let presence = live.phase;
  let success = false;
  let primary: { label: string; onClick: () => void | Promise<void>; loading?: boolean } | undefined;

  if (live.connection === "connecting") {
    title = "Looking for Ghost…";
  } else if (live.connection === "reconnecting") {
    title = "Waiting for Ghost";
    body = <p className="onb-try__line">It can’t hear you until it’s back.</p>;
  } else if (!live.online) {
    presence = "offline";
    title = "Ghost isn’t running";
    body = <p className="onb-try__line">It has to be running to hear its name.</p>;
    primary = inTauri ? { label: "Start Ghost", onClick: start, loading: starting } : { label: "How to start Ghost", onClick: () => w.go("service") };
  } else if (live.loadingModels) {
    presence = "starting";
    // The shell's banner already says the models are loading: here, only what it means for this step.
    title = `Say ${quoted(phrase)} in a moment`;
    body = <p className="onb-try__line">It starts listening for it as soon as its voice is ready.</p>;
  } else if (!w.done.models) {
    title = "Ghost needs its voice models";
    body = <p className="onb-try__line">It hears its name with models on this computer. Get them first; it takes a minute.</p>;
    primary = { label: "Get the voice models", onClick: () => w.go("models") };
  } else if (awake && heard === "voice") {
    success = true;
    title = "Ghost heard you";
    body = live.setupNeeded ? (
      <>
        <p className="onb-try__line">It woke on its name. Connect a model so it can answer.</p>
        <Button variant="secondary" size="sm" className="onb-try__action" onClick={() => w.go("model")}>
          Connect a model
        </Button>
      </>
    ) : (
      <p className="onb-try__line">
        Now ask it something, like {quoted("What needs my sign-off?")} Say {quoted("go to sleep")} when you’re done.
      </p>
    );
    primary = { label: "Continue", onClick: w.next };
  } else if (alreadyAwake) {
    title = "Ghost is already awake";
    body = (
      <>
        <p className="onb-try__line">
          Say {quoted("go to sleep")}, then try {quoted(phrase)}.
        </p>
        <Button variant="secondary" size="sm" className="onb-try__action" loading={sleeping} onClick={sleep}>
          Put it to sleep
        </Button>
      </>
    );
  } else if (awake) {
    title = "Ghost is awake";
    body = (
      <>
        <p className="onb-try__line">A click woke it, so the microphone isn’t checked yet. Say {quoted(phrase)} once it’s back to listening.</p>
        {live.setupNeeded && <p className="onb-try__meta">It can’t answer until it has a model; that’s step 2.</p>}
        <Button variant="secondary" size="sm" className="onb-try__action" loading={sleeping} onClick={sleep}>
          Put it to sleep
        </Button>
      </>
    );
  } else if (heard === "voice") {
    success = true;
    title = "Ghost heard you";
    body = <p className="onb-try__line">The microphone and the wake phrase both work. It’s back to listening for {quoted(phrase)}.</p>;
    primary = { label: "Continue", onClick: w.next };
  } else {
    title = `Say ${quoted(phrase)}`;
    body = (
      <>
        <p className="onb-try__line">
          {heard === "click" ? (
            <>The click woke it, but that doesn’t check the microphone. Say it once, the way you normally talk.</>
          ) : (
            <>Ghost is listening for it on {mic}. Nothing leaves this computer until it hears it.</>
          )}
        </p>
        {others.length > 0 && !quiet && <p className="onb-try__meta">It also answers to {others.map((o) => quoted(o)).join(" and ")}.</p>}
      </>
    );
  }
  const said = typeof title === "string" ? title : "";

  return (
    <StepFrame
      id="try"
      centered
      lead={<HeroPresence phase={presence} ok={success} />}
      title={title}
      primary={primary}
      onBack={w.back}
      onSkip={primary ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={
        error ? (
          <Notice tone="danger" title={error.title}>
            {error.text}
          </Notice>
        ) : undefined
      }
    >
      <p className="sr-only" aria-live="polite">
        {said}
      </p>
      <div className="onb-try">
        {body}
        {live.setupNeeded && ready && !awake && <p className="onb-try__meta">It can’t answer until it has a model; that’s step 2.</p>}
        {waiting && quiet && (
          <>
            <p className="onb-try__hint">
              Not hearing it?{" "}
              {device === "ghost.mic" ? (
                "Check that your microphone is on and not muted"
              ) : (
                <>
                  Check that <span className="t-mono">ghost.mic</span> is picked
                </>
              )}
              {others.length ? <>, or try {quoted(others[0])}</> : null}.
            </p>
            <div className="onb-try__actions">
              <Button variant="secondary" size="sm" onClick={() => w.go("microphone")}>
                Check the microphone
              </Button>
              <Button variant="ghost" size="sm" loading={waking} onClick={wake}>
                Wake it with a click
              </Button>
            </div>
          </>
        )}
        {waiting && !quiet && (
          <Button variant="secondary" size="sm" loading={waking} onClick={wake} className="onb-try__wake">
            Wake it with a click
          </Button>
        )}
      </div>
    </StepFrame>
  );
}
