/**
 * Owner: onboarding. PROTOTYPE: the first-run wizard at #/welcome.
 *
 * Eight steps in one place: welcome and the privacy promise, a model, GitHub (an app,
 * connected from Apps, ADR 0034), the microphone and echo
 * cancellation, the voice models, the background service, a first “Hey Ghost”, done. A rail on the left shows every step
 * and what's finished; the open step is the page's one focal point.
 *
 * Real endpoints where they exist (GET/PUT /settings, GET /devices, POST /setup/echo-cancel,
 * GET /setup/models, POST /wake, the live status). What the API can't do yet (report the mic
 * level, download models with progress) comes from the onboarding fixture and appears only
 * while the mock is on, tagged Prototype.
 *
 * Addresses: ?step=<id> before the hash opens a step (welcome, model, github, microphone,
 * models, service, try, done); ?state= picks a designed state while the mock is on
 * (models: idle, downloading, paused, failed, downloaded;
 * microphone: quiet; try: heard, nothear). Note that in firstrun the models are already on
 * disk: the download itself only shows with &state=idle, downloading, paused or failed.
 *
 * Layout: the rail is sticky to the panel's top. A step's footer (Back, Skip, the one primary)
 * sits a line under its content while the step fits the panel, and moves to the Page dock only
 * when the step overflows, so it stays in reach on the longest step at 1280x800.
 * Setup resumes from localStorage.
 *
 * One truth per fact: “Start Ghost” is done only when Ghost runs, has its models loaded and
 * the models step agrees they are on disk; “Say Hey Ghost” is done only when Ghost woke on
 * the operator’s voice, never on a click.
 */
import type { DevicesView, GhostClient, ModelsView, SettingsUpdate, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign, useFixture } from "../../design/context";
import type { OnboardingFixture } from "../../design/mock/fixtures/onboarding";
import { errorText, Page, scrollPanelToTop, useLive } from "../../ui";
import { DockContext, Rail } from "./frame";
import {
  demoState,
  download,
  formatMb,
  isStep,
  loadSaved,
  type Saved,
  STEPS,
  type StepId,
  stepFromUrl,
  stepIndex,
  storageKey,
  storeSaved,
  useDownload,
  writeStepToUrl,
} from "./state";
import { DoneStep, GitHubStep, MicrophoneStep, ModelStep, ModelsStep, ServiceStep, TryStep, WelcomeStep } from "./steps";
import "./onboarding.css";

export type SettingsResult = Awaited<ReturnType<GhostClient["updateSettings"]>>;

/** Everything a step needs. */
export interface Wizard {
  client: GhostClient;
  fixture: OnboardingFixture | null;
  settings: SettingsView | null;
  devices: DevicesView | null;
  models: ModelsView | null;
  /** First load hasn't answered yet. */
  loading: boolean;
  /** The last load failed (Ghost not running): steps that need the API say so. */
  loadError: string | null;
  reload: () => Promise<void>;
  /** PUT /settings, then reload. Remembers when a restart is needed. */
  save: (update: SettingsUpdate) => Promise<SettingsResult>;
  saved: Saved;
  setSaved: (patch: Partial<Saved>) => void;
  done: Record<StepId, boolean>;
  /** Steps whose state can't be read right now (Ghost isn't answering): shown as plain numbers, not as open. */
  unknown: StepId[];
  /** Online, models loaded and on disk: Ghost can hear its name. */
  voiceReady: boolean;
  /** The operator has been here before (resumed, or already configured when the wizard opened). */
  returning: boolean;
  /** The first task that isn't done, or "done". */
  firstOpen: () => StepId;
  next: () => void;
  back: () => void;
  skip: () => void;
  go: (id: StepId) => void;
  /** Move focus to the step's title when it opens. False on first load. */
  focusOnOpen: boolean;
  /** ?state= for a mock-only step's designed state. */
  demo: string | null;
}

export default function OnboardingPrototype({ params }: RouteProps) {
  const { client, status } = useGhost();
  const live = useLive();
  const design = useDesign();
  const fixture = useFixture<OnboardingFixture>("onboarding");
  const key = storageKey(design?.scenario.name ?? null);
  const demo = useMemo(() => demoState(), []);

  const [saved, setSavedState] = useState<Saved>(() => loadSaved(key));
  const resumed = useRef(!!saved.step && saved.step !== "welcome");
  const setSaved = useCallback(
    (patch: Partial<Saved>) =>
      setSavedState((s) => {
        const next = { ...s, ...patch };
        storeSaved(key, next);
        return next;
      }),
    [key],
  );

  // Open the step in the address, else the one saved last time, else the start.
  const [step, setStep] = useState<StepId>(() => (isStep(params.step) ? params.step : stepFromUrl()) ?? saved.step ?? "welcome");
  const [focusOnOpen, setFocusOnOpen] = useState(false);
  // The Page dock hosts every step's footer, so Back, Skip and Continue never leave the fold.
  const [dock, setDock] = useState<HTMLDivElement | null>(null);

  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [devices, setDevices] = useState<DevicesView | null>(null);
  const [models, setModels] = useState<ModelsView | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [sv, dv, mv] = await Promise.all([client.settings(), client.devices().catch(() => null), client.models().catch(() => null)]);
      setSettings(sv);
      setDevices(dv);
      setModels(mv);
      setLoadError(null);
    } catch (e) {
      setLoadError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, [client, design, demo]);

  // Load on open, again whenever Ghost comes back or finishes loading its models.
  useEffect(() => {
    void reload();
  }, [reload, live.online, status?.models_ready]);

  // The mock's model download: set up once from the fixture.
  useEffect(() => {
    if (!fixture) return;
    const designed = demo && ["idle", "downloading", "paused", "failed", "downloaded"].includes(demo) ? demo : null;
    download.init(fixture.models, fixture.downloadMbPerSec, designed ?? (fixture.modelsOnDisk ? "downloaded" : null));
  }, [fixture, demo]);
  const dl = useDownload();

  const save = useCallback(
    async (update: SettingsUpdate) => {
      const r = await client.updateSettings(update);
      if (r.restart_required) setSaved({ restartNeeded: true });
      await reload();
      return r;
    },
    [client, reload, setSaved],
  );

  // While Ghost is reconnecting, Start Ghost keeps its last known answer, like every other screen
  // keeps its last known data: the count doesn't flicker on a dropped connection. The rail says
  // “Reconnecting” under the step and the step's first row says the same, so both read as one fact.
  const serviceLive = live.online && !!status?.models_ready;
  const serviceKnown = useRef(serviceLive);
  if (live.connection !== "reconnecting") serviceKnown.current = serviceLive;
  const serviceUp = live.connection === "reconnecting" ? serviceKnown.current : serviceLive;

  // What's finished, read from Ghost itself wherever it can tell us.
  const done = useMemo<Record<StepId, boolean>>(() => {
    const v = settings?.values ?? {};
    const secret = (n: string) => settings?.secrets.find((s) => s.name === n)?.set ?? false;
    const modelsOnDisk = fixture ? dl.phase === "done" : !!models?.ready;
    const tasks = {
      welcome: true,
      model: !!settings && (secret("ANTHROPIC_AUTH_TOKEN") || !!v.ANTHROPIC_BASE_URL),
      // GitHub is an app (ADR 0034), connected from Apps. Until it is, a fresh install practices on sample issues.
      github: !!status?.apps.includes("github"),
      microphone: !!devices?.echo_cancel_active && !!v.GHOST_INPUT_DEVICE,
      models: modelsOnDisk,
      service: serviceUp && modelsOnDisk,
      try: saved.heard === "voice" || (!!fixture && demo === "heard"),
    };
    // Done is done when every task is.
    return { ...tasks, done: STEPS.every((s) => !s.task || tasks[s.id as keyof typeof tasks]) };
  }, [settings, devices, models, fixture, dl.phase, serviceUp, saved.heard, demo]);

  // Without an answer from Ghost, what's in its settings is unknown, not open. The first load
  // is the same: nothing is known yet, so nothing is numbered as open and there's no count.
  const checking = loading && !settings;
  const unknown = useMemo<StepId[]>(
    () => ((loadError || checking) && !settings ? ["model", "github", "microphone", ...(fixture ? [] : (["models"] as StepId[]))] : []),
    [loadError, checking, settings, fixture],
  );

  // “Welcome back” is decided once, from what was already true when the wizard opened.
  const [returning, setReturning] = useState<boolean | null>(null);
  useEffect(() => {
    if (returning !== null || !settings) return;
    setReturning(resumed.current || done.model || done.github);
  }, [returning, settings, done.model, done.github]);

  // Keep the address and the resume point in step with the wizard; clear the address on the way out.
  useEffect(() => {
    writeStepToUrl(step);
    setSaved({ step });
  }, [step, setSaved]);
  useEffect(() => () => writeStepToUrl(null), []);

  // A new step opens at the top of the panel.
  const go = useCallback((id: StepId) => {
    setFocusOnOpen(true);
    setStep(id);
    scrollPanelToTop();
  }, []);
  const offset = (d: number) => STEPS[Math.max(0, Math.min(STEPS.length - 1, stepIndex(step) + d))].id;
  const skippedRef = useRef(saved.skipped);
  skippedRef.current = saved.skipped;
  const next = () => {
    setSaved({ skipped: skippedRef.current.filter((s) => s !== step) });
    go(offset(1));
  };
  const back = () => go(offset(-1));
  const firstOpen = (): StepId => STEPS.find((s) => s.task && !done[s.id] && !unknown.includes(s.id))?.id ?? "done";
  const skip = () => {
    if (!skippedRef.current.includes(step)) setSaved({ skipped: [...skippedRef.current, step] });
    go(offset(1));
  };

  const w: Wizard = {
    client,
    fixture,
    settings,
    devices,
    models,
    loading,
    loadError,
    reload,
    save,
    saved,
    setSaved,
    done,
    unknown,
    voiceReady: live.online && !live.loadingModels && done.models,
    returning: !!returning || resumed.current,
    firstOpen,
    next,
    back,
    skip,
    go,
    focusOnOpen,
    demo,
  };

  // The rail repeats the download's header in short: the same numbers, never a second measure.
  const notes: Partial<Record<StepId, string>> = {};
  if (live.connection === "reconnecting") notes.service = "Reconnecting";
  if (fixture) {
    const total = fixture.models.reduce((a, m) => a + m.sizeMb, 0);
    const got = fixture.models.reduce((a, m) => a + Math.min(m.sizeMb, dl.got[m.id] ?? 0), 0);
    // The step says "stopped" once, in its Notice; the stepper only says how far it got.
    if (dl.phase === "downloading" || dl.phase === "failed") notes.models = `${formatMb(got)} of ${formatMb(total)}`;
    else if (dl.phase === "idle" && got > 0) notes.models = "Paused";
  }

  return (
    <Page bare width="default" title="Set up Ghost" dock={<div ref={setDock} className="onb-dock" />}>
      <DockContext.Provider value={dock}>
        <div className="onb">
          <Rail current={step} done={done} unknown={unknown} checking={checking} skipped={saved.skipped} notes={notes} onGo={go} />
          <div className="onb-main">
            {step === "welcome" && <WelcomeStep w={w} />}
            {step === "model" && <ModelStep w={w} />}
            {step === "github" && <GitHubStep w={w} />}
            {step === "microphone" && <MicrophoneStep w={w} />}
            {step === "models" && <ModelsStep w={w} />}
            {step === "service" && <ServiceStep w={w} />}
            {step === "try" && <TryStep w={w} />}
            {step === "done" && <DoneStep w={w} />}
          </div>
        </div>
      </DockContext.Provider>
    </Page>
  );
}
