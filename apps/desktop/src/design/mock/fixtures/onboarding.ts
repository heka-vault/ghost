/**
 * Owner: onboarding (prototype). First-run wizard data; read with useFixture("onboarding").
 *
 * Everything here stands in for something the control API can't do yet: report the live
 * microphone level, download the voice models with progress, and hear the wake phrase
 * during setup. Settings, devices, echo
 * cancellation and model status come from the mock daemon like everywhere else.
 */
import type { Scenario } from "../scenarios";

export type OnboardingStepId = "welcome" | "model" | "github" | "microphone" | "models" | "service" | "try" | "done";

export interface OnboardingStep {
  id: OnboardingStepId;
  title: string;
  description: string;
  done: boolean;
}

export interface OnboardingModel {
  id: "kokoro" | "parakeet";
  /** What it is to the operator. */
  title: string;
  /** What it is to the code, shown in mono. */
  detail: string;
  sizeMb: number;
}

export interface OnboardingFixture {
  steps: OnboardingStep[];
  /** Microphone input levels 0-1 for the meter, sampled every 100ms, looped. */
  micLevels: number[];
  suggestedWakePhrases: string[];
  /** The two speech models, about 1.6 GB together. */
  models: OnboardingModel[];
  /** Simulated download speed. Fast, so the whole download plays out in about 15 seconds. */
  downloadMbPerSec: number;
  /** The models are already in ~/.cache/ghost: the daemon has them ready, or the machine is configured. */
  modelsOnDisk: boolean;
}

export default function onboarding(s: Scenario): OnboardingFixture {
  const done = s.configured;
  // One truth with the daemon: if it reports its voice models ready, they are on disk. A
  // configured machine has them too, even while Ghost is down or still loading them. To design
  // the download itself, open the step with ?state=idle (or downloading, failed).
  const modelsOnDisk = s.modelsReady || s.configured;
  return {
    steps: [
      { id: "welcome", title: "Welcome", description: "What Ghost does and what stays on this computer.", done: true },
      { id: "model", title: "Connect a model", description: "An Anthropic-compatible endpoint and its token.", done },
      { id: "github", title: "Connect GitHub", description: "A fine‑grained token; the practice GitHub until then.", done },
      { id: "microphone", title: "Set up the microphone", description: "Echo cancellation and ghost.mic.", done },
      { id: "models", title: "Get the voice models", description: "Kokoro and Parakeet, about 1.6 GB.", done: modelsOnDisk },
      { id: "service", title: "Start Ghost", description: "The background service.", done: s.daemon && s.modelsReady && modelsOnDisk },
      { id: "try", title: "Say “Hey Ghost”", description: "Ghost hears its name on ghost.mic.", done: false },
      { id: "done", title: "Done", description: "", done: false },
    ],
    micLevels: [
      0.02, 0.03, 0.02, 0.03, 0.02, 0.04, 0.03, 0.02, 0.03, 0.02,
      0.06, 0.22, 0.46, 0.62, 0.38, 0.55, 0.71, 0.48, 0.3, 0.52,
      0.64, 0.44, 0.18, 0.08, 0.34, 0.58, 0.69, 0.41, 0.2, 0.06,
      0.03, 0.02, 0.03, 0.02, 0.02, 0.03,
    ],
    suggestedWakePhrases: ["hey ghost", "okay ghost", "ghost"],
    models: [
      { id: "kokoro", title: "Ghost’s voice", detail: "Kokoro 82M", sizeMb: 340 },
      { id: "parakeet", title: "Ghost’s ears", detail: "nemo-parakeet-tdt-0.6b-v3", sizeMb: 1260 },
    ],
    downloadMbPerSec: 110,
    modelsOnDisk,
  };
}
