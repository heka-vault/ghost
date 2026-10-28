/** Owner: onboarding. The account steps: welcome, model, GitHub, done. Voice steps live in voice.tsx. */
import { ArrowRight, Ear, FlaskConical, Hand, KeyRound, MessagesSquare, ShieldCheck } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { useGhost } from "@ghost/ui";

import { hrefFor, navigate } from "../../app/router";
import {
  Badge,
  Button,
  Card,
  Disclosure,
  Field,
  GhostMark,
  Input,
  ListRow,
  Notice,
  Skeleton,
  errorText,
  quoted,
  useLive,
} from "../../ui";
import type { Wizard } from "./index";
import { HeroPresence, StepFrame } from "./frame";
import { phraseCase, STEPS, type StepId, splitList } from "./state";

export { MicrophoneStep, ModelsStep, ServiceStep, TryStep } from "./voice";

/* ------------------------------------------------------------------------------ shared */

/** Set / Not set: all a secret ever shows. */
export function SecretBadge({ set }: { set: boolean }) {
  return set ? (
    <Badge tone="ok" size="sm" dot>
      Set
    </Badge>
  ) : (
    <Badge size="sm">Not set</Badge>
  );
}

/**
 * Shown on any step that saves through Ghost while Ghost isn't running, pointing at the step
 * that starts it. While reconnecting the shell's banner already says so: nothing here.
 */
export function NeedsGhost({ w, what = "save this" }: { w: Wizard; what?: string }) {
  const { connection } = useLive();
  if (connection !== "offline") return null;
  return (
    <Notice
      tone="warn"
      title="Ghost isn’t running yet"
      action={
        <Button size="sm" onClick={() => w.go("service")}>
          Start Ghost
        </Button>
      }
    >
      Ghost has to be running to {what}. Start it, then come back here.
    </Notice>
  );
}

/** Under the footer while reconnecting with unsaved changes: why Continue is off. */
export function useSavesWhenBack(dirty: boolean): string | undefined {
  const { connection } = useLive();
  return connection === "reconnecting" && dirty ? "Saves when Ghost is back" : undefined;
}

/** Field skeletons while there is no answer from GET /settings yet: the first load, or Ghost not running. Unknown looks unknown. */
function FieldsSkeleton({ n = 2 }: { n?: number }) {
  return (
    <div className="onb-fields" aria-hidden>
      {Array.from({ length: n }, (_, i) => (
        <div key={i} className="onb-skel-field">
          <Skeleton width={120} height={12} />
          <Skeleton height={32} radius="var(--radius-md)" />
        </div>
      ))}
    </div>
  );
}

/* ----------------------------------------------------------------------------- welcome */

/** The three promises. The copy follows the reader: a model already connected, keys in a vault. */
function promises(w: Wizard): { icon: ReactNode; title: string; body: ReactNode }[] {
  const connected = w.returning || w.done.model;
  return [
    {
      icon: <Ear />,
      title: "It listens only for its name",
      body: (
        <>
          Until you say “Hey Ghost”, audio stays with speech models on this computer and is thrown away. After that, your words go to{" "}
          {connected ? "the model you connected" : "your model"}.
        </>
      ),
    },
    {
      icon: <Hand />,
      title: "It asks before it changes anything",
      body: <>Ghost reads issues freely. Closing, labeling or commenting waits for your spoken yes, until you decide otherwise.</>,
    },
    {
      icon: <KeyRound />,
      title: "Your keys stay with you",
      body: w.settings?.vault_managed ? (
        <>Your vault provides them each time Ghost starts. This app never sees them.</>
      ) : (
        <>
          Tokens are saved in <span className="t-mono">~/.config/ghost/env</span>, readable only by you. This app can set them, but never shows them again.
        </>
      ),
    },
  ];
}

export function WelcomeStep({ w }: { w: Wizard }) {
  const back = w.returning;
  return (
    <StepFrame
      id="welcome"
      lead={<GhostMark size={40} />}
      title={back ? "Welcome back" : "Welcome to Ghost"}
      description={
        back
          ? "Ghost goes through your GitHub issues with you, out loud, and records what you decide. Pick up where you left off."
          : "Ghost goes through your GitHub issues with you, out loud, and records what you decide. Setup takes about ten minutes. Stop whenever you like; it picks up here."
      }
      primary={back ? { label: "Continue setup", onClick: () => w.go(w.firstOpen()) } : { label: "Get started", onClick: w.next }}
      autoFocus={w.focusOnOpen}
    >
      <ul className="onb-promises" aria-label="What Ghost promises">
        {promises(w).map((p) => (
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

export function ModelStep({ w }: { w: Wizard }) {
  const { online } = useLive();
  const s = w.settings;
  const tokenSet = s?.secrets.find((x) => x.name === "ANTHROPIC_AUTH_TOKEN")?.set ?? false;
  const [base, setBase] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);

  const vault = !!s?.vault_managed;
  const tokenInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (missing) tokenInput.current?.focus();
  }, [missing]);
  const baseValue = base ?? s?.values.ANTHROPIC_BASE_URL ?? "";
  const modelValue = model ?? s?.values.GHOST_LLM_MODEL ?? "";

  const dirty = !!token.trim() || base !== null || model !== null;
  const footNote = useSavesWhenBack(dirty);

  const submit = async () => {
    if (!s) return;
    setError(null);
    const values: Record<string, string | null> = {};
    if (s && baseValue.trim() !== (s.values.ANTHROPIC_BASE_URL ?? "")) values.ANTHROPIC_BASE_URL = baseValue.trim() || null;
    if (s && modelValue.trim() !== (s.values.GHOST_LLM_MODEL ?? "")) values.GHOST_LLM_MODEL = modelValue.trim() || null;
    const secrets = token.trim() ? { ANTHROPIC_AUTH_TOKEN: token.trim() } : null;
    const changed = Object.keys(values).length > 0 || !!secrets;
    if (!changed) {
      if (w.done.model) return w.next();
      setMissing(true);
      return;
    }
    if (!baseValue.trim() && !tokenSet && !secrets) {
      setMissing(true);
      return;
    }
    setBusy(true);
    try {
      await w.save({ values: Object.keys(values).length ? values : null, secrets });
      setToken("");
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
      title="Connect a model"
      description={
        vault
          ? "Ghost thinks with a large language model. On this computer, your vault provides the key, so there’s nothing to type."
          : "Ghost thinks with a large language model. Give it an endpoint that speaks Anthropic’s API, and the token for it."
      }
      primary={{ label: "Continue", onClick: submit, loading: busy, disabled: (w.loading && !s) || (!online && (!w.done.model || dirty)) }}
      onBack={w.back}
      onSkip={w.done.model ? undefined : w.skip}
      footNote={footNote}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} what="save a token" />
          {error && (
            <Notice tone="danger" title="Couldn’t save">
              {error}
            </Notice>
          )}
        </>
      }
    >
      {!s ? (
        <FieldsSkeleton />
      ) : vault ? (
        <>
          <Card list>
            <ListRow
              title="Model token"
              description={<span className="t-mono">ANTHROPIC_AUTH_TOKEN</span>}
              trailing={
                <Badge tone="ok" dot>
                  From your vault
                </Badge>
              }
            />
            {s?.values.ANTHROPIC_BASE_URL && <ListRow title="Endpoint" description={<span className="t-mono">{s.values.ANTHROPIC_BASE_URL}</span>} />}
            {s?.values.GHOST_LLM_MODEL && <ListRow title="Model" description={<span className="t-mono">{s.values.GHOST_LLM_MODEL}</span>} />}
          </Card>
          <p className="onb-help onb-after-card">Ghost reads it from the vault each time it starts. To change it, change it there.</p>
        </>
      ) : (
        <div className="onb-fields">
          <Field label="Endpoint" detail="ANTHROPIC_BASE_URL" optional help="Leave it empty for Anthropic itself. A local proxy or another provider works too.">
            <Input value={baseValue} placeholder="https://api.anthropic.com" inputMode="url" spellCheck={false} onChange={(e) => (setBase(e.target.value), setMissing(false))} />
          </Field>
          <Field
           
            label="Token"
            detail="ANTHROPIC_AUTH_TOKEN"
            aside={<SecretBadge set={tokenSet} />}
            help={tokenSet ? "Paste a new one only to replace it." : "Saved on this computer only. You won’t see it again."}
            error={missing ? "Paste a token, or an endpoint that doesn’t need one. Or skip this for now." : undefined}
          >
            <Input
              ref={tokenInput}
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={token}
              placeholder={tokenSet ? "Replace…" : "sk-ant-api03-…"}
              onChange={(e) => (setToken(e.target.value), setMissing(false))}
            />
          </Field>
          <Disclosure summary="Choose a model" defaultOpen={!!s?.values.GHOST_LLM_MODEL}>
            <Field label="Model" detail="GHOST_LLM_MODEL" optional help="Leave it empty for the default.">
              <Input value={modelValue} placeholder="claude-sonnet-5-5" spellCheck={false} onChange={(e) => setModel(e.target.value)} />
            </Field>
          </Disclosure>
        </div>
      )}
    </StepFrame>
  );
}

/* ------------------------------------------------------------------------------ github */

/**
 * GitHub is an app (ADR 0034): it connects from the catalog under Apps, by signing in, and
 * every agent reaches it. This step only says whether it is connected and
 * takes the operator there; there is no token or repository list here. Keeping Ghost to some
 * repositories is a rule on Permissions.
 */
export function GitHubStep({ w }: { w: Wizard }) {
  const { status } = useGhost();
  const connected = !!status?.apps.includes("github");
  return (
    <StepFrame
      id="github"
      title="Connect GitHub"
      description="Ghost reaches GitHub as an app. Sign in to it from Apps, or keep practicing on sample issues for now."
      primary={{ label: "Continue", onClick: () => w.next() }}
      onBack={w.back}
      onSkip={w.done.github ? undefined : w.skip}
      autoFocus={w.focusOnOpen}
      notices={<NeedsGhost w={w} what="connect an app" />}
    >
      <div className="onb-fields onb-fields--loose">
        <div>
          <Card list>
            {connected ? (
              <ListRow leading={<KeyRound size={16} aria-hidden />} title="GitHub is connected" description="Ghost can look things up and make changes there, and tells you what it changed." />
            ) : (
              <ListRow
                leading={<FlaskConical size={16} aria-hidden />}
                title="Using the practice GitHub for now"
                description="Sample issues, kept in memory. Nothing reaches github.com until you connect GitHub."
              />
            )}
            <ListRow
              leading={<ShieldCheck size={16} aria-hidden />}
              title="Keep Ghost to some repositories"
              description={
                <>
                  Add a rule on Permissions, such as <span className="t-mono">github.*(repo!=app)</span>. Nothing written in an issue can change it.
                </>
              }
            />
          </Card>
          <div className="onb-links">
            <Button size="sm" variant="secondary" href={hrefFor("/settings/apps")} trailingIcon={<ArrowRight />}>
              {connected ? "Open Apps" : "Connect GitHub in Apps"}
            </Button>
            <Button size="sm" variant="ghost" href={hrefFor("/settings/permissions")}>
              Open Permissions
            </Button>
          </div>
        </div>
      </div>
    </StepFrame>
  );
}

/* -------------------------------------------------------------------------------- done */

export function DoneStep({ w }: { w: Wizard }) {
  const live = useLive();
  const s = w.settings;
  const v = s?.values ?? {};
  const phrase = phraseCase(splitList(v.GHOST_WAKE_PHRASES)[0] ?? "hey ghost");

  // Why each open step is open, in one line.
  const why: Partial<Record<StepId, ReactNode>> = {
    model: "Without one, Ghost can’t answer.",
    github: "Using the practice GitHub for now.",
    microphone: !w.devices?.echo_cancel_active ? "Echo cancellation isn’t installed yet." : (
      <>
        Not listening on <span className="t-mono">ghost.mic</span> yet.
      </>
    ),
    models: "About 1.6 GB, not downloaded yet.",
    service: !live.online ? "Ghost isn’t running." : !w.done.models ? "Waiting for the voice models." : "Loading the voice models.",
    try: w.saved.heard === "click" ? "Woken with a click · not heard yet" : "Not tried yet.",
  };
  const tasks = STEPS.filter((x) => x.task);
  const open = tasks.filter((x) => !w.done[x.id]);
  const all = open.length === 0;

  if (all)
    return (
      <StepFrame
        id="done"
        centered
        lead={<HeroPresence phase={live.phase} size={96} ok />}
        title="You’re set"
        description={`Say ${quoted(phrase)} whenever you need it. Ghost asks before it changes anything on GitHub, until you tell it otherwise.`}
        primary={{ label: "Go to the session", onClick: () => navigate("/") }}
        onBack={w.back}
        autoFocus={w.focusOnOpen}
      >
        <div className="onb-next">
          <p className="onb-next__label t-small t-3">When you’re ready</p>
          <Card list>
            <ListRow
              leading={<ShieldCheck size={16} aria-hidden />}
              title="Set permissions"
              description="Ghost uses its apps and tells you what it changed. Block a tool, or have it ask first."
              href={hrefFor("/settings/permissions")}
              chevron
            />
            <ListRow
              leading={<MessagesSquare size={16} aria-hidden />}
              title="Add a conversation"
              description="A line with its own persona, voice and memory."
              href={hrefFor("/lines")}
              chevron
            />
          </Card>
        </div>
      </StepFrame>
    );

  return (
    <StepFrame
      id="done"
      title="Almost there"
      description={`${open.length === 1 ? "One thing is" : `${NUMBER[open.length] ?? open.length} things are`} still open. Ghost can wait; finish ${open.length === 1 ? "it" : "them"} whenever you like.`}
      primary={{ label: "Go to the session", onClick: () => navigate("/") }}
      onBack={w.back}
      autoFocus={w.focusOnOpen}
    >
      <Card list>
        {open.map((t) => (
          <ListRow
            key={t.id}
            title={t.label}
            description={w.unknown.includes(t.id) ? (w.loading ? "Checking…" : "Start Ghost to check this.") : why[t.id]}
            trailing={
              <Button size="sm" variant="secondary" onClick={() => w.go(t.id)}>
                {t.id === "try" ? (w.saved.heard === "click" ? "Try saying it" : "Try it") : "Set up"}
              </Button>
            }
          />
        ))}
      </Card>
    </StepFrame>
  );
}

const NUMBER: Record<number, string> = { 2: "Two", 3: "Three", 4: "Four", 5: "Five", 6: "Six" };
