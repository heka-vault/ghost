/** Owner: onboarding. The account steps: welcome, model, GitHub, done. Voice steps live in voice.tsx. */
import { Ear, FlaskConical, FolderGit2, Hand, KeyRound, MessagesSquare, Plus, ShieldCheck } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { hrefFor, navigate } from "../../app/router";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Disclosure,
  Field,
  GhostMark,
  Input,
  ListRow,
  Notice,
  RadioGroup,
  Skeleton,
  errorText,
  quoted,
  scrollIntoPanel,
  useLive,
} from "../../ui";
import type { Wizard } from "./index";
import { HeroPresence, PrototypeTag, Row, StepFrame } from "./frame";
import { phraseCase, STEPS, type StepId, splitList, truthy } from "./state";

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

type GitHubMode = "practice" | "token";
/** Stands in for a pasted token in the ?state=looking / found screenshots (mock only; it only ever reaches the mock daemon). */
const DEMO_TOKEN = "github_pat_prototype";
/** My GitHub: the repository list comes from the token, so it appears only once there is one. */
type Lookup = "none" | "looking" | "done";

export function GitHubStep({ w }: { w: Wizard }) {
  const { online } = useLive();
  const s = w.settings;
  const tokenSet = s?.secrets.find((x) => x.name === "GITHUB_TOKEN")?.set ?? false;
  const practiceOn = truthy(s?.values.GHOST_GITHUB_MOCK);
  const savedRepos = useMemo(() => splitList(s?.values.GHOST_REPOS), [s]);

  // ?state=looking / found: My GitHub with a token pasted, mid-lookup or after it, for screenshots.
  const demoToken = !!w.fixture && (w.demo === "looking" || w.demo === "found");
  const [mode, setMode] = useState<GitHubMode | null>(demoToken ? "token" : null);
  const modeValue: GitHubMode = mode ?? (tokenSet && !practiceOn ? "token" : "practice");
  const [token, setToken] = useState(demoToken ? DEMO_TOKEN : "");
  const [picked, setPicked] = useState<string[] | null>(null);
  const [typed, setTyped] = useState<string | null>(null);
  const [extra, setExtra] = useState("");
  const [extraError, setExtraError] = useState(false);
  const [added, setAdded] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tokenMissing, setTokenMissing] = useState(false);
  const [reposMissing, setReposMissing] = useState(false);
  const tokenInput = useRef<HTMLInputElement>(null);
  const tokenField = useRef<HTMLDivElement>(null);
  const reposBox = useRef<HTMLFieldSetElement>(null);

  // Prototype: a pasted token “looks up” its repositories for a moment, then the list appears.
  const hasToken = tokenSet || !!token.trim();
  const [lookup, setLookup] = useState<Lookup>("none");
  useEffect(() => {
    if (!w.fixture || modeValue !== "token") return;
    if (demoToken && token === DEMO_TOKEN) return setLookup(w.demo === "looking" ? "looking" : "done");
    if (tokenSet) return setLookup("done");
    if (!token.trim()) return setLookup("none");
    setLookup("looking");
    const t = setTimeout(() => setLookup("done"), 1100);
    return () => clearTimeout(t);
  }, [w.fixture, modeValue, tokenSet, token, demoToken, w.demo]);

  const fullList = w.fixture ? (modeValue === "practice" ? w.fixture.practiceRepos : w.fixture.availableRepos) : null;
  const list = fullList && (modeValue === "practice" || lookup === "done") ? fullList : fullList ? [] : null;
  // Practice repositories are harmless: start with them all ticked so the default just works.
  const defaultPicked = savedRepos.length ? savedRepos : modeValue === "practice" && fullList ? fullList.map((r) => r.name) : [];
  const pickedValue = picked ?? defaultPicked;
  const knownNames = new Set(list?.map((r) => r.name) ?? []);
  const extras = [...new Set([...savedRepos.filter((r) => !knownNames.has(r) && !(modeValue === "practice" && fullList?.some((x) => x.name === r))), ...added])];
  const repos = fullList ? pickedValue.filter((r) => knownNames.has(r) || extras.includes(r)) : splitList(typed ?? s?.values.GHOST_REPOS ?? "");

  const dirty = !!token.trim() || mode !== null || picked !== null || typed !== null;
  const footNote = useSavesWhenBack(dirty);

  const toggle = (name: string, on: boolean) => {
    setPicked(on ? [...pickedValue.filter((r) => r !== name), name] : pickedValue.filter((r) => r !== name));
    setReposMissing(false);
  };
  const addExtra = () => {
    const name = extra.trim().replace(/^https:\/\/github\.com\//, "").replace(/\/+$/, "");
    if (!/^[\w.-]+\/[\w.-]+$/.test(name)) return void setExtraError(true);
    setExtraError(false);
    setAdded((a) => [...a, name]);
    toggle(name, true);
    setExtra("");
  };

  const submit = async () => {
    if (!s) return;
    setError(null);
    const needToken = modeValue === "token" && !tokenSet && !token.trim();
    setTokenMissing(needToken);
    // One problem at a time: the repositories are only asked for once the token is sorted.
    setReposMissing(!needToken && repos.length === 0);
    // Focus the first thing to fix, as the model step does, and bring its message into view:
    // after a mouse click the focus ring doesn't draw, so the words have to be on screen.
    if (needToken) {
      tokenInput.current?.focus({ preventScroll: true });
      scrollIntoPanel(tokenField.current, { block: "nearest" });
      return;
    }
    if (repos.length === 0) {
      reposBox.current?.querySelector<HTMLInputElement>("input:not([disabled])")?.focus({ preventScroll: true });
      // To the panel's end: the whole picker, past the dock.
      scrollIntoPanel(reposBox.current, { block: "panel-end" });
      return;
    }
    const values: Record<string, string | null> = {};
    if (practiceOn !== (modeValue === "practice")) values.GHOST_GITHUB_MOCK = modeValue === "practice" ? "1" : null;
    const joined = repos.join(",");
    if (joined !== savedRepos.join(",")) values.GHOST_REPOS = joined;
    const secrets = modeValue === "token" && token.trim() ? { GITHUB_TOKEN: token.trim() } : null;
    if (!Object.keys(values).length && !secrets) return w.next();
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

  const rows = [...(list ?? []).map((r) => ({ ...r, extra: false })), ...extras.map((name) => ({ name, private: false, openIssues: -1, extra: true }))];

  return (
    <StepFrame
      id="github"
      title="Connect GitHub"
      description="Start with the practice GitHub to get a feel for Ghost, or connect your own. Either way, Ghost only touches the repositories you pick."
      primary={{ label: "Continue", onClick: submit, loading: busy, disabled: (w.loading && !s) || (!online && (!w.done.github || dirty)) }}
      onBack={w.back}
      onSkip={w.done.github ? undefined : w.skip}
      footNote={footNote}
      autoFocus={w.focusOnOpen}
      notices={
        <>
          <NeedsGhost w={w} />
          {error && (
            <Notice tone="danger" title="Couldn’t save">
              {error}
            </Notice>
          )}
        </>
      }
    >
      {!s ? (
        <FieldsSkeleton n={3} />
      ) : (
        <div className="onb-fields onb-fields--loose">
          <RadioGroup<GitHubMode>
            aria-label="Which GitHub"
            variant="card"
            columns={2}
            value={modeValue}
            onChange={(m) => {
              setMode(m);
              setPicked(null);
              setReposMissing(false);
              setTokenMissing(false);
            }}
            options={[
              { value: "practice", label: "Practice GitHub", icon: <FlaskConical />, description: "Sample issues, kept in memory. Nothing reaches github.com." },
              { value: "token", label: "My GitHub", icon: <FolderGit2 />, description: "Your real issues, through a fine‑grained token." },
            ]}
          />

          {modeValue === "token" && (
            <div ref={tokenField} className="onb-fields">
            <Field
              label="GitHub token"
              detail="GITHUB_TOKEN"
              aside={<SecretBadge set={tokenSet} />}
              help={
                tokenSet
                  ? "Paste a new one only to replace it."
                  : "Fine‑grained, with read and write access to issues on the repositories you pick. Saved on this computer only."
              }
              error={tokenMissing ? "Paste a token, or use the practice GitHub for now." : undefined}
            >
              <Input
                ref={tokenInput}
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={token}
                placeholder={tokenSet ? "Replace…" : "github_pat_11A…"}
                onChange={(e) => (setToken(e.target.value), setTokenMissing(false))}
              />
            </Field>
            </div>
          )}

          {list ? (
            <fieldset
              ref={reposBox}
              className="onb-repos"
              aria-describedby={reposMissing ? "onb-repos-error onb-repos-help" : "onb-repos-help"}
              aria-invalid={reposMissing || undefined}
              aria-busy={lookup === "looking" || undefined}
            >
              <legend className="onb-repos__legend">
                <span className="onb-repos__label">Repositories Ghost may touch</span>
                <span className="t-mono t-3 t-small">GHOST_REPOS</span>
                <PrototypeTag />
              </legend>
              {reposMissing && (
                <p id="onb-repos-error" className="onb-help onb-help--error onb-repos__error" role="alert">
                  {modeValue === "token" && !rows.length ? "Add at least one repository by name, like ghost-mock/app." : "Pick at least one repository."}
                </p>
              )}
              <Card list>
                {modeValue === "token" && !hasToken && (
                  <Row className="onb-repo onb-repo--note">
                    <p className="t-body t-2">Paste a token to see the repositories it can reach.</p>
                  </Row>
                )}
                {modeValue === "token" && lookup === "looking" && (
                  <>
                    <Row className="onb-repo onb-repo--note">
                      <p className="t-body t-2" role="status">
                        Looking up your repositories…
                      </p>
                    </Row>
                    {[160, 128, 144].map((width) => (
                      <Row key={width} className="onb-repo onb-repo--skel">
                        <Skeleton width={16} height={16} radius="var(--radius-xs)" />
                        <div className="onb-repo__skel-text">
                          <Skeleton width={width} height={12} />
                          <Skeleton width={96} height={10} />
                        </div>
                      </Row>
                    ))}
                  </>
                )}
                {rows.map((r) => (
                  <Row key={r.name} className="onb-repo">
                    <Checkbox
                      checked={pickedValue.includes(r.name)}
                      onChange={(on) => toggle(r.name, on)}
                      label={r.name}
                      description={
                        r.extra
                          ? "Added by name"
                          : `${r.openIssues} open ${r.openIssues === 1 ? "issue" : "issues"}${modeValue === "token" ? ` · ${r.private ? "private" : "public"}` : ""}`
                      }
                    />
                  </Row>
                ))}
                {modeValue === "token" && (
                  <Row className="onb-repo-add">
                    <Input
                      size="sm"
                      aria-label="Add a repository by name"
                      placeholder="owner/repo"
                      spellCheck={false}
                      value={extra}
                      invalid={extraError}
                      aria-describedby={extraError ? "onb-repo-add-error" : undefined}
                      onChange={(e) => (setExtra(e.target.value), setExtraError(false))}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          addExtra();
                        }
                      }}
                    />
                    <Button size="sm" variant="ghost" leadingIcon={<Plus />} onClick={addExtra} disabled={!extra.trim()}>
                      Add
                    </Button>
                    {extraError && (
                      <p id="onb-repo-add-error" className="onb-help onb-help--error onb-repo-add__error" role="alert">
                        Use owner/repo, like ghost-mock/app.
                      </p>
                    )}
                  </Row>
                )}
              </Card>
              <p id="onb-repos-help" className="onb-help">
                Ghost reads and changes issues only here. Nothing written in an issue can add to this list.
              </p>
            </fieldset>
          ) : (
            <Field
             
              label="Repositories Ghost may touch"
              detail="GHOST_REPOS"
              help="owner/repo, separated by commas. Nothing written in an issue can add to this list."
              error={reposMissing ? "Add at least one repository, like ghost-mock/app." : undefined}
            >
              <Input
                value={typed ?? s?.values.GHOST_REPOS ?? ""}
                placeholder="ghost-mock/app, ghost-mock/infra"
                spellCheck={false}
                onChange={(e) => (setTyped(e.target.value), setReposMissing(false))}
              />
            </Field>
          )}
        </div>
      )}
    </StepFrame>
  );
}

/* -------------------------------------------------------------------------------- done */

export function DoneStep({ w }: { w: Wizard }) {
  const live = useLive();
  const s = w.settings;
  const v = s?.values ?? {};
  const secret = (n: string) => s?.secrets.find((x) => x.name === n)?.set ?? false;
  const repoCount = splitList(v.GHOST_REPOS).length;
  const phrase = phraseCase(splitList(v.GHOST_WAKE_PHRASES)[0] ?? "hey ghost");

  // Why each open step is open, in one line.
  const why: Partial<Record<StepId, ReactNode>> = {
    model: "Without one, Ghost can’t answer.",
    github: truthy(v.GHOST_GITHUB_MOCK) || secret("GITHUB_TOKEN") ? (repoCount ? "Saved" : "No repositories picked yet.") : "No GitHub connected yet.",
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
              title="Decide what Ghost may do"
              description="Let it close or label without asking, or never."
              href={hrefFor("/policy")}
              chevron
            />
            <ListRow
              leading={<MessagesSquare size={16} aria-hidden />}
              title="Add a conversation"
              description="A line with its own persona, voice and memory."
              href={hrefFor("/conversations")}
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
