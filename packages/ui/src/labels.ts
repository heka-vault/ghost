import type { Autonomy, Phase } from "@ghost/client-ts";

export function phaseLabel(phase: Phase): string {
  return {
    starting: "Starting",
    passive: "Waiting for “Hey Ghost”",
    listening: "Listening",
    thinking: "Thinking",
    speaking: "Speaking",
  }[phase];
}

export function autonomyLabel(level: Autonomy | string): string {
  return (
    {
      deny: "Not allowed",
      confirm: "Ask me first",
      announce: "Do it and tell me",
      quiet: "Do it quietly",
    }[level] ?? level
  );
}
