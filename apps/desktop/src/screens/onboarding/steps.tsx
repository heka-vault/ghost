/**
 * Owner: onboarding. The first run's steps, from nothing to a first spoken answer: the model
 * and its check, the microphone, the wake heard, the voice, an app, an agent, a question.
 * Every step reads what is true from Ghost and saves through it; none promises what Ghost
 * hasn't said.
 */
import type { CatalogEntryView, ConversationDetail, ModelConnectionView } from "@ghost/client-ts";
import { lookOf, useGhost } from "@ghost/ui";
import { ArrowRight, AudioLines, Check, Cpu, Ear, Hand, KeyRound, Mic, MicOff, Plus, Search, Volume2, WifiOff } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { notListening } from "../../app/listening";
import { hrefFor, navigate } from "../../app/router";
import { inTauri, startDaemon } from "../../transport";
import {
  AgentShapeGlyph,
  Badge,
  Button,
  Card,
  cx,
  errorText,
  GhostMark,
  Input,
  LineGlyph,
  ListRow,
  Notice,
  quoted,
  RadioGroup,
  Select,
  Skeleton,
  useLive,
} from "../../ui";
import { AddOwnDialog } from "../apps/AddOwnDialog";
import { AppIcon } from "../apps/AppIcon";
import { ConnectDialog } from "../apps/ConnectDialog";
import { NewAgentDialog } from "../conversations/NewAgentDialog";
import { verdictOf } from "../settings/check";
import { GhostVoice } from "../settings/GhostVoice";
import { CONNECTION_KEYS, micOptions, validateConnection } from "../settings/model";
import { ModelSection } from "../settings/ModelSection";
import { HeroPresence, StepFrame } from "./frame";
import type { Wizard } from "./index";
import { firstSentence, inputName, phraseCase, splitList, wakeHelp } from "./state";
import { KEEP_NOTICE } from "../hear/wakeTakes";
import { exchangeOf, isAwake, useWakeWatch } from "./watch";

/* ------------------------------------------------------------------------------ shared */

/** The operator turned listening off (ADR 0057): the microphone is closed. */
function useMicOff(): boolean {
  const { status } = useGhost();
  return !!notListening(status);
}

/** On a step that saves through Ghost while Ghost isn't running: start it here, or say how. */
function NeedsGhost({ w, what }: { w: Wizard; what: string }) {
  const { connection } = useLive();
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (connection !== "offline") return null;
  return (
    <Notice
      tone="danger"
      title="Ghost isn’t running"
      action={
        inTauri ? (
          <Button
            size="sm"
            loading={starting}
            onClick={async () => {
              setStarting(true);
              setError(null);
              try {
                await startDaemon();
              } catch (e) {
                setError(errorText(e));
              } finally {
                setStarting(false);
              }
            }}
          >
            Start Ghost
          </Button>
        ) : undefined
      }
    >
      {error ?? (
        <>
          It has to be running to {what}.{" "}
          {!inTauri && (
            <>
              Start it with <span className="t-mono">systemctl --user start ghostd</span>.
            </>
          )}
        </>
      )}
    </Notice>
  );
}

/** A step already done says so in one line, and Continue stays the way on. */
function DoneLine({ children }: { children: ReactNode }) {
  return (
    <p className="onb-doneline">
      <Check size={14} aria-hidden /> {children}
    </p>
  );
}

/* ----------------------------------------------------------------------------- welcome */

const PROMISES = [
  { icon: <Ear />, title: "It listens only for its name", body: "Until it hears it, audio stays on this computer and is thrown away." },
  { icon: <Hand />, title: "It does the work and tells you", body: "Anything it should ask about first, or never do, is one rule in Permissions." },
  { icon: <KeyRound />, title: "Your passwords stay with you", body: "In its own vault, locked with your keyring. Never shown again." },
];

export function WelcomeStep({ w }: { w: Wizard }) {
  const back = w.returning;
  return (
    <StepFrame
      id="welcome"
      lead={<GhostMark size={40} />}
      title={back ? "Welcome back" : "Welcome to Ghost"}
      description={back ? "Pick up where you left off." : "Seven short steps to your first spoken answer. Skip any of them; setup picks up where you stop."}
      primary={back ? { label: "Continue setup", onClick: () => w.go(w.progress.next) } : { label: "Get started", onClick: w.next }}
      autoFocus={w.focusOnOpen}
    >
      <ul className="onb-promises" aria-label="What Ghost promises">
        {PROMISES.map((p) => (
          <li key={p.title} className="onb-promise">
            <span className="onb-tile" aria-hidden>
              {p.icon}
            </span>
            <div>
              <p className="onb-promise__title">{p.title}</p>
              <p className="onb-promise__body">{p.body}</p>
            </div>
          </li>
        ))}
      </ul>
    </StepFrame>
  );
}

/* ------------------------------------------------------------------------------- model */

/**
 * Settings › Model's own section, on a draft of its own: any provider, its key in the vault,
 * its model from the provider's list, and Test, which checks the draft before anything is
 * saved. Continue saves the draft.
 */
export function ModelStep({ w }: { w: Wizard }) {
  const { client } = useGhost();
  const { online } = useLive();
  const design = !!w.design;
  const [conn, setConn] = useState<ModelConnectionView | null>(null);
  const [values, setValuesState] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const s = w.settings;

  const readConn = () => {
    client.modelConnection().then(setConn, () => setConn(null));
  };
  useEffect(readConn, [client, online]);
  // The draft starts as what is saved.
  useEffect(() => {
    if (s) setValuesState(Object.fromEntries(CONNECTION_KEYS.map((k) => [k, s.values[k] ?? ""])));
  }, [s]);

  const problems = validateConnection(values);
  const dirty = !!s && CONNECTION_KEYS.some((k) => (values[k] ?? "") !== (s.values[k] ?? ""));
  const submit = async () => {
    if (!dirty) return w.next();
    if (Object.keys(problems).length) return setError(Object.values(problems)[0]);
    setBusy(true);
    setError(null);
    try {
      await w.save({ values: Object.fromEntries(CONNECTION_KEYS.filter((k) => (values[k] ?? "") !== (s?.values[k] ?? "")).map((k) => [k, values[k]?.trim() || null])) });
      readConn();
      w.next();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <StepFrame
      id="model"
      plain
      title="Connect a model"
      description="Where Ghost thinks. Any provider works, a model on this computer too; Test says how well it will answer out loud."
      primary={{ label: dirty ? "Save and continue" : "Continue", onClick: submit, loading: busy, disabled: !online }}
      onBack={w.back}
      onSkip={w.progress.state.model === "done" ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="connect a model" />
          {error && (
            <Notice tone="danger" title="Couldn’t save" onDismiss={() => setError(null)}>
              {error}
            </Notice>
          )}
        </>
      }
    >
      {!s ? (
        <Card aria-hidden padding="md">
          <Skeleton height={14} width={220} />
        </Card>
      ) : (
        <div className="onb-model">
          <ModelSection
            s={s}
            conn={conn}
            values={values}
            online={online}
            mock={design}
            setValues={(patch) => setValuesState((v) => ({ ...v, ...patch }))}
            help={(_, t) => t}
            err={(k) => problems[k] ?? null}
            fieldId={(k) => `onb-${k}`}
            waiting={new Set()}
            onPassword={() => {
              readConn();
              void w.reload();
            }}
          />
        </div>
      )}
    </StepFrame>
  );
}

/** The kept check in one line, for the done step and the rail. */
export function useModelVerdict(): string | null {
  const { client } = useGhost();
  const { online } = useLive();
  const [line, setLine] = useState<string | null>(null);
  useEffect(() => {
    if (!online) return;
    client
      .modelCheckLatest()
      .then((r) => setLine(r.check ? verdictOf(r.check).title : null))
      .catch(() => setLine(null));
  }, [client, online]);
  return line;
}

/* -------------------------------------------------------------------------------- hear */

/** Where the operator's voice goes, as a picture: each stop lit once it is true. */
function VoicePath({ stops }: { stops: { icon: ReactNode; label: string; detail: string; ok: boolean | null }[] }) {
  return (
    <ol className="onb-path" aria-label="Where your voice goes">
      {stops.map((s, i) => (
        <li key={s.label} className={cx("onb-path__stop", s.ok === true && "is-ok", s.ok === false && "is-open")}>
          <span className="onb-path__mark" aria-hidden>
            {s.icon}
          </span>
          <span className="onb-path__label">{s.label}</span>
          <span className="onb-path__detail">{s.detail}</span>
          <span className="sr-only">{s.ok === true ? ", ready" : s.ok === false ? ", not yet" : ", can’t check yet"}</span>
          {i < stops.length - 1 && <span className="onb-path__wire" aria-hidden />}
        </li>
      ))}
    </ol>
  );
}

export function HearStep({ w }: { w: Wizard }) {
  const live = useLive();
  const d = w.devices;
  const saved = w.settings?.values.GHOST_INPUT_DEVICE ?? "";
  const [device, setDevice] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // ADR 0064: with the daemon's reading of the machine, unset lets Ghost choose by its case
  // (an array that cancels on chip heard directly, a headset as it is, ghost.mic where the
  // canceller runs); without it, ghost.mic when it is there, as before.
  const mp = d?.microphone ?? null;
  const value = device ?? (saved || (mp ? "" : d?.echo_cancel_source_present ? "ghost.mic" : ""));
  const noCancellerNeeded = mp?.case === "on-chip" || mp?.case === "headset";
  const echo = !!d?.echo_cancel_active || noCancellerNeeded;
  const speech = w.models?.ready ?? null;
  const off = useMicOff();

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      const r = await w.client.setupEchoCancel(false);
      if (!r.ok) setError(r.detail ?? "The installer stopped without saying why. Run ghostd setup echo-cancel in a terminal to see it.");
      // A device that cancels echo itself: the installer says so and installs nothing.
      else if (/no host echo canceller is needed/.test(r.detail ?? "")) setError(null);
      await w.reload();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setInstalling(false);
    }
  };
  const submit = async () => {
    if (value === saved) return w.next();
    setBusy(true);
    setError(null);
    try {
      await w.save({ values: { GHOST_INPUT_DEVICE: value || null } });
      w.next();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const micLabel = value || (mp ? `Ghost’s choice: ${inputName(mp.session_input)}` : "System default");
  return (
    <StepFrame
      id="hear"
      title="Let Ghost hear you"
      description="Everything here runs on this computer. Nothing is sent anywhere until Ghost hears its name."
      primary={{ label: "Continue", onClick: submit, loading: busy, disabled: !live.online && value !== saved }}
      onBack={w.back}
      onSkip={w.progress.state.hear === "done" ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="set up the microphone" />
          {error && (
            <Notice tone="danger" title="That didn’t work" onDismiss={() => setError(null)}>
              {error}
            </Notice>
          )}
        </>
      }
    >
      <VoicePath
        stops={[
          { icon: off ? <MicOff /> : <Mic />, label: off ? "Not listening" : "Your microphone", detail: off ? "Turned off" : micLabel, ok: d ? !off : null },
          { icon: <AudioLines />, label: "Its own voice taken out", detail: noCancellerNeeded ? (mp?.case === "headset" ? "A headset" : "By the microphone itself") : d?.echo_cancel_active ? "ghost.mic" : "Not set up", ok: d ? echo : null },
          { icon: <Ear />, label: "Listens for its name", detail: speech ? "On this computer" : "Models not here yet", ok: speech },
          { icon: <Cpu />, label: "Your model", detail: "Only after its name", ok: w.progress.state.model === "done" ? true : w.progress.state.model === "unknown" ? null : false },
        ]}
      />
      {!d ? (
        <Card list aria-hidden>
          <div className="onb-row">
            <Skeleton width={180} height={14} />
            <Skeleton height={32} radius="var(--radius-md)" />
          </div>
        </Card>
      ) : (
        <Card list>
          {off && (
            <ListRow
              title="Listening is off"
              description="The microphone is closed, so Ghost can’t hear its name. Typing still works."
              trailing={
                <Button size="sm" onClick={() => void w.client.setListening(true).catch((e) => setError(errorText(e)))}>
                  Turn listening on
                </Button>
              }
            />
          )}
          {mp && (
            <ListRow
              title="How Ghost hears you"
              description={
                <>
                  {mp.summary}
                  {mp.advice ? ` ${mp.advice}` : ""}
                  {mp.fell_back ? ` ${mp.fell_back}` : ""}
                </>
              }
            />
          )}
          <ListRow
            title="Take Ghost’s voice out of the microphone"
            description={
              noCancellerNeeded
                ? "Not needed here: Ghost’s voice doesn’t come back through this microphone."
                : echo
                  ? "Running: PipeWire removes what Ghost says from what it hears."
                  : "One click: adds ghost.mic, so Ghost doesn’t hear itself. Headphones help too."
            }
            trailing={
              noCancellerNeeded ? (
                <Badge tone="ok" dot>
                  Not needed
                </Badge>
              ) : echo ? (
                <Badge tone="ok" dot>
                  Running
                </Badge>
              ) : (
                <Button size="sm" loading={installing} disabled={!live.online} onClick={install}>
                  Set it up
                </Button>
              )
            }
          />
          <div className="onb-row">
            <label className="onb-label" htmlFor="onb-mic">
              Microphone <span className="onb-detail">GHOST_INPUT_DEVICE</span>
            </label>
            <Select id="onb-mic" value={value} onChange={(e) => setDevice(e.target.value)} options={micOptions(d, value)} />
          </div>
          <ListRow
            title="Speech models"
            description={speech ? "Ghost’s ears and voice are on this computer." : "Ghost downloads them once, about 1.6 GB, when it starts. After that speech never needs the internet."}
            trailing={speech ? <Badge tone="ok" dot>Here</Badge> : speech === false ? <Badge>Downloading at start</Badge> : <Skeleton width={80} height={20} />}
          />
        </Card>
      )}
    </StepFrame>
  );
}

/* -------------------------------------------------------------------------------- wake */

/** How long Ghost may stay quiet, ready and listening, before the step offers help. */
const NOT_HEARD_AFTER_MS = 12_000;

export function WakeStep({ w }: { w: Wizard }) {
  const live = useLive();
  const [waking, setWaking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const phrases = splitList(w.settings?.values.GHOST_WAKE_PHRASES);
  const list = phrases.length ? phrases : ["hey ghost"];
  const phrase = phraseCase(list[0]);
  const awake = isAwake(live.online, live.phase);
  const heard = w.saved.heard ?? (w.demo === "heard" ? "voice" : null);
  const ready = live.online && !live.loadingModels && w.models?.ready !== false;
  const fuzzy = Number(w.settings?.values.GHOST_WAKE_FUZZY || 0.9);
  const off = useMicOff();

  const { alreadyAwake, markClick } = useWakeWatch(awake, (how, at) => {
    if (w.saved.heard !== "voice") w.setSaved({ heard: how, heardAt: at.toISOString() });
  });

  // Ready and nothing heard for a while: what to try, from what is true now.
  const [quiet, setQuiet] = useState(w.demo === "nothear");
  const waiting = ready && !awake && heard !== "voice";
  useEffect(() => {
    if (!waiting || w.demo === "nothear") return;
    setQuiet(false);
    const t = setTimeout(() => setQuiet(true), NOT_HEARD_AFTER_MS);
    return () => clearTimeout(t);
  }, [waiting, w.demo]);
  const ladder = wakeHelp({ listeningOff: off, device: w.settings?.values.GHOST_INPUT_DEVICE, echo: !!w.devices?.echo_cancel_active, fuzzy, phrases: list });

  const wake = async () => {
    setWaking(true);
    setError(null);
    markClick();
    try {
      await w.client.wake();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setWaking(false);
    }
  };
  const fix = async (id: string) => {
    setError(null);
    try {
      if (id === "listen") await w.client.setListening(true);
      else if (id === "mic") w.go("hear");
      else if (id === "fuzzy") await w.save({ values: { GHOST_WAKE_FUZZY: "0.8" } });
      else if (id === "hearing") navigate("/hear");
    } catch (e) {
      setError(errorText(e));
    }
  };
  const FIX_LABEL: Record<string, string> = { listen: "Turn listening on", mic: "Check the microphone", fuzzy: "Match more loosely", hearing: "Open the Hearing test" };

  let title: string;
  let line: ReactNode = null;
  let presence = live.phase;
  let success = false;
  if (!live.online) {
    presence = "offline";
    title = "Ghost isn’t running";
    line = "It has to be running to hear its name.";
  } else if (off) {
    title = "Ghost isn’t listening";
    line = "The microphone is off. Turn listening on and say its name.";
  } else if (live.loadingModels || w.models?.ready === false) {
    presence = "starting";
    title = `Say ${quoted(phrase)} in a moment`;
    line = "Its ears are still loading.";
  } else if (heard === "voice") {
    success = true;
    title = "Ghost heard you";
    line = w.saved.heardAt ? `It woke on its name at ${new Date(w.saved.heardAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}, on this computer.` : "It woke on its name, on this computer.";
  } else if (alreadyAwake) {
    title = "Ghost is already awake";
    line = `Say ${quoted("that’s all")}, then try ${quoted(phrase)}.`;
  } else if (awake) {
    title = "Awake, from a click";
    line = `A click doesn’t check the microphone. Once it’s back to listening, say ${quoted(phrase)}.`;
  } else {
    title = `Say ${quoted(phrase)}`;
    line = heard === "click" ? "Out loud this time, the way you normally talk." : "Ghost lights up the moment it hears it.";
  }

  return (
    <StepFrame
      id="wake"
      centered
      lead={<HeroPresence phase={presence} ok={success} />}
      title={title}
      primary={success ? { label: "Continue", onClick: w.next } : undefined}
      onBack={w.back}
      onSkip={success ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="hear its name" />
          {error && (
            <Notice tone="danger" title="That didn’t work" onDismiss={() => setError(null)}>
              {error}
            </Notice>
          )}
        </>
      }
    >
      <p className="sr-only" aria-live="polite">
        {title}
      </p>
      <div className="onb-try">
        {line && <p className="onb-try__line">{line}</p>}
        <p className="onb-try__meta">
          {KEEP_NOTICE}{" "}
          <a href="#/settings/sound" onClick={(e) => (e.preventDefault(), navigate("/settings/sound"))}>
            Turn off or delete
          </a>
        </p>
        {list.length > 1 && !success && !quiet && <p className="onb-try__meta">It also answers to {list.slice(1).map((o) => quoted(phraseCase(o))).join(" and ")}.</p>}
        {(quiet || off) && !success && (
          <div className="onb-ladder">
            <p className="onb-ladder__head">Not hearing it? Try these, in order:</p>
            <ol className="onb-ladder__list">
              {ladder.map((step) => (
                <li key={step.id}>
                  <span>{step.text}</span>
                  {FIX_LABEL[step.id] && (
                    <Button size="sm" variant="secondary" onClick={() => void fix(step.id)}>
                      {FIX_LABEL[step.id]}
                    </Button>
                  )}
                </li>
              ))}
            </ol>
          </div>
        )}
        {waiting && !off && (
          <Button variant="ghost" size="sm" loading={waking} onClick={wake} className="onb-try__wake">
            Wake it with a click instead
          </Button>
        )}
      </div>
    </StepFrame>
  );
}

/* ------------------------------------------------------------------------------- voice */

type Mode = "local" | "realtime";

export function VoiceStep({ w }: { w: Wizard }) {
  const { client } = useGhost();
  const live = useLive();
  const savedMode: Mode = w.settings?.values.GHOST_VOICE_MODE === "realtime" ? "realtime" : "local";
  const [mode, setMode] = useState<Mode>(savedMode);
  useEffect(() => setMode(savedMode), [savedMode]);
  const [ghostVoice, setGhostVoice] = useState<{ voice: string; speed: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    client.conversation("ghost").then(
      (d) => setGhostVoice({ voice: d.voice, speed: d.speed }),
      () => setGhostVoice(null),
    );
  }, [client, live.online]);
  const pickVoice = async (voice: string) => {
    setError(null);
    setGhostVoice((g) => (g ? { ...g, voice } : g));
    try {
      await client.updateConversation("ghost", { voice });
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <StepFrame
      id="voice"
      title="Choose its voice"
      description="How Ghost hears and speaks to you. You can change it any time in Settings › Sound."
      primary={
        mode === "local"
          ? { label: "Continue", onClick: () => (w.setSaved({ voiceChosen: true }), w.next()) }
          : { label: "Set it up in Settings › Sound", onClick: () => (w.setSaved({ voiceChosen: true }), navigate("/settings/sound")) }
      }
      onBack={w.back}
      autoFocus={w.focusOnOpen}
      notices={
        error ? (
          <Notice tone="danger" title="Couldn’t change the voice" onDismiss={() => setError(null)}>
            {error}
          </Notice>
        ) : undefined
      }
    >
      <RadioGroup<Mode>
        variant="card"
        aria-label="Voice mode"
        value={mode}
        onChange={setMode}
        options={[
          { value: "local", label: "On this computer", description: "Private and free, and works without the internet. Ghost’s own voice.", icon: <Volume2 />, aside: savedMode === "local" ? <Badge size="sm">In use</Badge> : undefined },
          { value: "realtime", label: "A realtime voice", description: "Quicker small talk from OpenAI’s realtime model. Costs a little per minute.", icon: <AudioLines />, aside: savedMode === "realtime" ? <Badge size="sm">In use</Badge> : undefined },
        ]}
      />
      {mode === "realtime" && savedMode !== "realtime" && (
        <Notice tone="info" title="What changes with a realtime voice" className="onb-after-card">
          While a session is open at your desk, its audio goes to OpenAI, never before Ghost hears its name, never on hold and never on the phone. Every read and change is still Ghost’s, through your permissions. It needs an OpenAI password, and you turn it on in Settings › Sound, where all of this is shown before anything changes.
        </Notice>
      )}
      {mode === "local" && (
        <div className="onb-voice">
          <p className="onb-label">Ghost’s voice</p>
          {ghostVoice ? <GhostVoice value={ghostVoice.voice} speed={ghostVoice.speed} onChange={(v) => void pickVoice(v)} /> : <Skeleton height={48} />}
        </div>
      )}
    </StepFrame>
  );
}

/* --------------------------------------------------------------------------------- app */

export function AppStep({ w }: { w: Wizard }) {
  const { client } = useGhost();
  const live = useLive();
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<CatalogEntryView[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [open, setOpen] = useState<CatalogEntryView | null>(null);
  const [own, setOwn] = useState(false);
  const connected = (w.apps ?? []).filter((a) => a.name !== "ghost-mock-github");

  useEffect(() => {
    if (!live.online) return;
    let alive = true;
    const t = window.setTimeout(() => {
      client
        .catalog(query, 0, query ? 8 : 6)
        .then((p) => {
          if (!alive) return;
          setEntries(p.entries);
          setProblem(p.entries.length ? null : p.problem ?? (p.refreshing ? "Ghost is fetching the catalog." : null));
        })
        .catch((e) => alive && (setEntries([]), setProblem(errorText(e))));
    }, query ? 250 : 0);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [client, query, live.online]);

  return (
    <StepFrame
      id="app"
      title="Connect an app"
      description="What Ghost works in: your issues, your calendar, your mail. Every agent reaches what you connect, under your permissions."
      primary={connected.length ? { label: "Continue", onClick: w.next } : undefined}
      onBack={w.back}
      onSkip={connected.length ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={<NeedsGhost w={w} what="connect an app" />}
    >
      {connected.length > 0 && (
        <div className="onb-chips" aria-label="Connected">
          {connected.map((a) => (
            <span key={a.name} className="onb-chip">
              <AppIcon title={a.title} icon={a.icon} size="sm" /> {a.title}
              <Check size={14} aria-label="connected" className="t-ok" />
            </span>
          ))}
        </div>
      )}
      <Input leading={<Search />} value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search apps, like GitHub or Calendar…" aria-label="Search apps" />
      {problem && !entries?.length ? (
        <Notice tone="info" title="The catalog isn’t here" icon={<WifiOff />} className="onb-after-card">
          {problem} It needs the internet once; skip this and connect apps from Apps later, or add your own below.
        </Notice>
      ) : (
        <Card list className="onb-after-card">
          {!entries
            ? Array.from({ length: 4 }, (_, i) => (
                <div key={i} className="onb-row" aria-hidden>
                  <Skeleton width={200} height={14} />
                </div>
              ))
            : entries.map((e) => (
                <ListRow
                  key={e.name}
                  leading={<AppIcon title={e.title} icon={e.icon} />}
                  title={e.title}
                  description={firstSentence(e.description)}
                  trailing={
                    e.connected.length ? (
                      <Badge tone="ok" dot>
                        Connected
                      </Badge>
                    ) : (
                      <Button size="sm" variant="secondary" disabled={!live.online} onClick={() => setOpen(e)}>
                        Connect…
                      </Button>
                    )
                  }
                />
              ))}
        </Card>
      )}
      <Button variant="ghost" size="sm" leadingIcon={<Plus />} className="onb-own" onClick={() => setOwn(true)} disabled={!live.online}>
        Add your own, by address or command…
      </Button>
      {open && (
        <ConnectDialog
          entry={open}
          onClose={() => setOpen(null)}
          onConnected={() => {
            setOpen(null);
            void w.reload();
          }}
        />
      )}
      <AddOwnDialog
        open={own}
        onClose={() => setOwn(false)}
        onConnected={() => {
          setOwn(false);
          void w.reload();
        }}
      />
    </StepFrame>
  );
}

/* ------------------------------------------------------------------------------- agent */

export function AgentStep({ w }: { w: Wizard }) {
  const live = useLive();
  const [open, setOpen] = useState(false);
  const [made, setMade] = useState<ConversationDetail | null>(null);
  const agents = (w.agents ?? []).filter((a) => !a.is_ghost && !a.retired_at);
  const latest = made ?? null;
  const shown = latest ? lookOf(latest.slug, latest.look) : null;

  return (
    <StepFrame
      id="agent"
      title="Hire an agent"
      description="An agent is someone with a job: its own name, persona, voice, memory and shape. Ghost hands work to it; you can call it by name."
      primary={agents.length ? { label: "Continue", onClick: w.next } : { label: "Hire an agent…", onClick: () => setOpen(true), disabled: !live.online, icon: <Plus /> }}
      secondary={
        agents.length ? (
          <Button variant="ghost" leadingIcon={<Plus />} onClick={() => setOpen(true)} disabled={!live.online}>
            Hire another…
          </Button>
        ) : undefined
      }
      onBack={w.back}
      onSkip={agents.length ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={<NeedsGhost w={w} what="hire an agent" />}
    >
      {latest && shown ? (
        <div className="onb-hired">
          <span className="onb-hired__mark" style={{ color: `var(--hue-${shown.hue}-fg)`, background: `var(--hue-${shown.hue}-bg)` }} aria-hidden>
            <AgentShapeGlyph shape={shown.shape} size={72} />
          </span>
          <div>
            <p className="onb-hired__name">{latest.name} is on the rail</p>
            <p className="onb-hired__job">{latest.job || "No job yet: give it one on its page."}</p>
            <p className="onb-hired__job">
              Say {quoted(`Ghost, transfer me to ${latest.name}`)}, or pick it on the rail.
            </p>
          </div>
        </div>
      ) : agents.length ? (
        <Card list>
          {agents.slice(0, 5).map((a) => (
            <ListRow key={a.slug} leading={<LineGlyph slug={a.slug} name={a.name} size="lg" />} title={a.name} description={a.job || "No job yet"} href={hrefFor(`/agents/${a.slug}`)} />
          ))}
        </Card>
      ) : (
        <div className="onb-shapes" aria-hidden>
          {(["drop", "bell", "hood", "cloud", "lantern", "star"] as const).map((sh, i) => (
            <span key={sh} className="onb-shapes__one" style={{ color: `var(--hue-${i + 1}-fg)`, background: `var(--hue-${i + 1}-bg)` }}>
              <AgentShapeGlyph shape={sh} size={36} />
            </span>
          ))}
        </div>
      )}
      <NewAgentDialog
        open={open}
        onClose={() => setOpen(false)}
        taken={(w.agents ?? []).map((a) => a.name.toLowerCase())}
        onCreated={(d) => {
          setOpen(false);
          setMade(d);
          void w.reload();
        }}
      />
    </StepFrame>
  );
}

/* --------------------------------------------------------------------------------- ask */

const SUGGESTIONS = ["What can you do?", "What needs me today?", "What did you change today?"];

export function AskStep({ w }: { w: Wizard }) {
  const { client, turns } = useGhost();
  const live = useLive();
  const since = useRef(Date.now());
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const spoke = useRef(false);
  if (live.phase === "speaking") spoke.current = true;
  const ex = useMemo(() => exchangeOf(turns, since.current), [turns]);
  const answered = !!ex || w.saved.answered;

  useEffect(() => {
    if (ex && !w.saved.answered) w.setSaved({ answered: true });
  }, [ex, w]);

  const send = async (t: string) => {
    const q = t.trim();
    if (!q) return;
    setSending(true);
    setError(null);
    try {
      await client.say(q);
      setText("");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSending(false);
    }
  };

  const blocked = w.progress.state.model !== "done";
  const phrase = phraseCase(splitList(w.settings?.values.GHOST_WAKE_PHRASES)[0] ?? "hey ghost");
  return (
    <StepFrame
      id="ask"
      centered
      lead={<HeroPresence phase={live.online ? live.phase : "offline"} ok={answered} size={96} />}
      title={ex ? "Ghost answered" : answered ? "Ghost has answered you" : blocked ? "Connect a model first" : `Say ${quoted(`${phrase}, what can you do?`)}`}
      description={!ex && !answered && !blocked ? "Or type it. Ghost answers out loud either way." : undefined}
      primary={answered ? { label: "Continue", onClick: w.next } : blocked ? { label: "Connect a model", onClick: () => w.go("model") } : undefined}
      onBack={w.back}
      onSkip={answered ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="answer" />
          {error && (
            <Notice tone="danger" title="Couldn’t ask" onDismiss={() => setError(null)}>
              {error}
            </Notice>
          )}
        </>
      }
    >
      {ex ? (
        <div className="onb-exchange" data-user-content>
          <p className="onb-exchange__q">
            {ex.spoken ? <Mic size={14} aria-label="Said" /> : null} {ex.question}
          </p>
          <p className="onb-exchange__a">{ex.answer}</p>
          <p className="onb-try__meta">{spoke.current ? "Said out loud, in Ghost’s voice." : "Ghost says its answers out loud while a session is open."}</p>
        </div>
      ) : (
        !blocked && (
          <div className="onb-ask">
            <div className="onb-ask__chips">
              {SUGGESTIONS.map((s) => (
                <Button key={s} size="sm" variant="secondary" disabled={!live.online || sending} onClick={() => void send(s)}>
                  {s}
                </Button>
              ))}
            </div>
            <form
              className="onb-ask__form"
              onSubmit={(e) => {
                e.preventDefault();
                void send(text);
              }}
            >
              <Input value={text} onChange={(e) => setText(e.target.value)} placeholder="Ask Ghost something…" aria-label="Ask Ghost something" disabled={!live.online} />
              <Button type="submit" variant="secondary" loading={sending} disabled={!live.online || !text.trim()} trailingIcon={<ArrowRight />}>
                Ask
              </Button>
            </form>
          </div>
        )
      )}
    </StepFrame>
  );
}

/* -------------------------------------------------------------------------------- done */

const FIX: Partial<Record<string, string>> = {
  model: "Ghost can’t answer without one.",
  hear: "Ghost may hear itself, or not hear you.",
  wake: "Not yet heard on your voice.",
  voice: "Ghost uses its voice on this computer.",
  app: "Ghost has only its own tools.",
  agent: "Ghost does everything itself.",
  ask: "Ask whenever you like.",
};

export function DoneStep({ w }: { w: Wizard }) {
  const verdict = useModelVerdict();
  const open = w.tasks.filter((t) => w.progress.state[t.id] !== "done");
  const all = open.length === 0;
  const finish = (to: string) => {
    w.setSaved({ finished: true });
    navigate(to);
  };
  return (
    <StepFrame
      id="done"
      centered
      lead={<HeroPresence phase={all ? "passive" : "passive"} ok={all} />}
      title={all ? "Ghost is ready" : w.progress.canTalk ? "Ghost can talk with you" : "Setup is saved"}
      description={all ? `Say ${quoted("Hey Ghost")} whenever you need it.` : "What isn’t set up yet is below. Each one is a click from here, or from Settings."}
      primary={{ label: "Go to Today", onClick: () => finish("/") }}
      autoFocus={w.focusOnOpen}
    >
      <Card list className="onb-summary">
        {w.tasks.map((t) => {
          const st = w.progress.state[t.id];
          return (
            <ListRow
              key={t.id}
              leading={
                <span className={cx("onb-mark", st === "done" && "is-done", st === "skipped" && "is-skipped")} aria-hidden>
                  {st === "done" ? <Check size={12} strokeWidth={2.5} /> : "–"}
                </span>
              }
              title={t.label}
              description={st === "done" ? (t.id === "model" && verdict ? verdict : "Done") : st === "unknown" ? "Can’t check while Ghost isn’t running" : `${st === "skipped" ? "Skipped. " : "Not set up. "}${FIX[t.id] ?? ""}`}
              trailing={
                st !== "done" ? (
                  <Button size="sm" variant="ghost" onClick={() => w.go(t.id)}>
                    Set up
                  </Button>
                ) : undefined
              }
            />
          );
        })}
      </Card>
    </StepFrame>
  );
}
