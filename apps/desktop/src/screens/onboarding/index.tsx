/**
 * Owner: onboarding. The first run at #/welcome: from nothing to a first spoken answer.
 *
 * Seven tasks: a model and its check (any provider), the microphone and Ghost's own voice taken
 * out of it, its name heard on the operator's voice, how it speaks, an app, an agent with a
 * shape, and a question answered aloud. Each is done only when Ghost says so (`progressOf`),
 * each can be skipped, and setup resumes from where it stopped (localStorage, and the step in
 * the address). The steps that need nothing from outside this computer work without a network;
 * the rail says which one needs it. Real endpoints only: nothing here exists only in the mock.
 */
import type { AppView, ConversationView, DevicesView, GhostClient, ModelsView, SettingsUpdate, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Page, scrollPanelToTop, useLive } from "../../ui";
import { DockContext, Rail } from "./frame";
import {
  demoState,
  type Facts,
  isStep,
  loadSaved,
  type Progress,
  progressOf,
  type Saved,
  type StepDef,
  STEPS,
  type StepId,
  stepFromUrl,
  stepIndex,
  storageKey,
  storeSaved,
  TASKS,
  writeStepToUrl,
} from "./state";
import { AgentStep, AppStep, AskStep, DoneStep, HearStep, ModelStep, VoiceStep, WakeStep, WelcomeStep } from "./steps";
import "../settings/settings.css";
import "./onboarding.css";

export type SettingsResult = Awaited<ReturnType<GhostClient["updateSettings"]>>;

/** Everything a step needs. */
export interface Wizard {
  client: GhostClient;
  design: ReturnType<typeof useDesign>;
  settings: SettingsView | null;
  devices: DevicesView | null;
  models: ModelsView | null;
  apps: AppView[] | null;
  agents: ConversationView[] | null;
  reload: () => Promise<void>;
  /** PUT /settings, then reload. */
  save: (update: SettingsUpdate) => Promise<SettingsResult>;
  saved: Saved;
  setSaved: (patch: Partial<Saved>) => void;
  progress: Progress;
  tasks: StepDef[];
  /** The operator has been here before. */
  returning: boolean;
  next: () => void;
  back: () => void;
  skip: () => void;
  go: (id: StepId) => void;
  /** Move focus to the step's title when it opens. False on first load. */
  focusOnOpen: boolean;
  /** ?state= for a designed state under the mock. */
  demo: string | null;
}

export default function Onboarding({ params }: RouteProps) {
  const { client, status } = useGhost();
  const live = useLive();
  const design = useDesign();
  const key = storageKey(design?.scenario.name ?? null);
  const demo = useMemo(() => (import.meta.env.DEV && design ? demoState() : null), [design]);

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

  const [step, setStep] = useState<StepId>(() => (isStep(params.step) ? params.step : stepFromUrl()) ?? saved.step ?? "welcome");
  const [focusOnOpen, setFocusOnOpen] = useState(false);
  const [dock, setDock] = useState<HTMLDivElement | null>(null);

  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [devices, setDevices] = useState<DevicesView | null>(null);
  const [models, setModels] = useState<ModelsView | null>(null);
  const [apps, setApps] = useState<AppView[] | null>(null);
  const [agents, setAgents] = useState<ConversationView[] | null>(null);
  const [modelReady, setModelReady] = useState<boolean | null>(null);
  const [loaded, setLoaded] = useState(false);

  // Each read on its own: one that fails (no audio devices, no catalog) leaves the others.
  const reload = useCallback(async () => {
    const [sv, dv, mv, av, cv, mc] = await Promise.all([
      client.settings().catch(() => null),
      client.devices().catch(() => null),
      client.models().catch(() => null),
      client.apps().catch(() => null),
      client.conversations().catch(() => null),
      client.modelConnection().catch(() => null),
    ]);
    setSettings(sv);
    setDevices(dv);
    setModels(mv);
    setApps(av);
    setAgents(cv);
    setModelReady(mc ? mc.ready : null);
    setLoaded(true);
  }, [client]);

  // On open, and again whenever Ghost comes back, finishes loading, or apps and agents change.
  useEffect(() => {
    void reload();
  }, [reload, live.online, status?.models_ready, status?.apps.length]);

  const save = useCallback(
    async (update: SettingsUpdate) => {
      const r = await client.updateSettings(update);
      await reload();
      return r;
    },
    [client, reload],
  );

  const facts: Facts = useMemo(
    () =>
      live.online
        ? {
            modelReady,
            modelChecked: null,
            speechReady: models ? models.ready : null,
            echo: devices ? devices.echo_cancel_active : null,
            apps: apps ? apps.filter((a) => a.state === "connected" && a.name !== "ghost-mock-github").length : null,
            agents: agents ? agents.filter((a) => !a.is_ghost && !a.retired_at).length : null,
            voiceSet: settings ? !!settings.values.GHOST_VOICE_MODE : null,
          }
        : { modelReady: null, modelChecked: null, speechReady: null, echo: null, apps: null, agents: null, voiceSet: null },
    [live.online, modelReady, models, devices, apps, agents, settings],
  );
  const progress = useMemo(() => progressOf(facts, saved), [facts, saved]);

  const [returning, setReturning] = useState<boolean | null>(null);
  useEffect(() => {
    if (returning !== null || !loaded) return;
    setReturning(resumed.current || progress.state.model === "done");
  }, [returning, loaded, progress.state.model]);

  useEffect(() => {
    writeStepToUrl(step);
    setSaved({ step });
  }, [step, setSaved]);
  useEffect(() => () => writeStepToUrl(null), []);

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
  const skip = () => {
    if (!skippedRef.current.includes(step)) setSaved({ skipped: [...skippedRef.current, step] });
    go(offset(1));
  };

  const w: Wizard = {
    client,
    design,
    settings,
    devices,
    models,
    apps,
    agents,
    reload,
    save,
    saved,
    setSaved,
    progress,
    tasks: TASKS,
    returning: !!returning || resumed.current,
    next,
    back,
    skip,
    go,
    focusOnOpen,
    demo,
  };

  return (
    <Page bare width="default" title="Set up Ghost" dock={<div ref={setDock} className="onb-dock" />}>
      <DockContext.Provider value={dock}>
        <div className="onb">
          <Rail current={step} progress={progress} checking={!loaded} onGo={go} />
          <div className="onb-main">
            {step === "welcome" && <WelcomeStep w={w} />}
            {step === "model" && <ModelStep w={w} />}
            {step === "hear" && <HearStep w={w} />}
            {step === "wake" && <WakeStep w={w} />}
            {step === "voice" && <VoiceStep w={w} />}
            {step === "app" && <AppStep w={w} />}
            {step === "agent" && <AgentStep w={w} />}
            {step === "ask" && <AskStep w={w} />}
            {step === "done" && <DoneStep w={w} />}
          </div>
        </div>
      </DockContext.Provider>
    </Page>
  );
}
