/**
 * Owner: settings. The model check (ADR 0056) as one verdict and what would help, so Settings ›
 * Model and the first run say the same thing about the same check. Pure and tested.
 */
import type { ModelCheckModel, ModelCheckView } from "@ghost/client-ts";

/** Past this a spoken reply's first word is late enough to be felt (`ghost_llm::check::SLOW_FIRST_WORD_SECS`). */
export const SLOW_FIRST_WORD_MS = 1500;

export interface Verdict {
  /** Never amber: amber means only “waiting for your okay”. */
  tone: "ok" | "danger" | "info";
  /** One line, the answer to “will it work when I talk to it?”. */
  title: string;
  /** What would help, as a sentence; null when nothing would. */
  help: string | null;
  /** The main model's first word, for a figure beside the verdict. */
  firstWordMs: number | null;
}

const ms = (n: number) => (n < 1000 ? `${n} ms` : `${(n / 1000).toFixed(1)} s`);
const sentence = (t: string) => {
  const s = t.trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) + (/[.!?]$/.test(s) ? "" : ".") : s;
};

export function verdictOf(r: ModelCheckView): Verdict {
  const main: ModelCheckModel | undefined = r.models.find((m) => m.role !== "fast") ?? r.models[0];
  const fast = r.models.find((m) => m.role === "fast");
  if (!main) return { tone: "danger", title: "Nothing was checked", help: "Pick a model, then Test again.", firstWordMs: null };
  if (!main.reachable) {
    return {
      tone: "danger",
      title: "Ghost can’t reach the model",
      help: `${sentence(main.error ?? "No answer came back.")} Check the address, the model’s name and its password, then Test again.`,
      firstWordMs: null,
    };
  }
  const fw = main.first_word.median_ms ?? null;
  if (main.tools.state === "no") {
    return {
      tone: "danger",
      title: "It answers, but can’t use tools",
      help: "Ghost reaches your apps and makes changes through tool calls. Pick a model that takes them.",
      firstWordMs: fw,
    };
  }
  const spoken = fast?.reachable ? fast.first_word.median_ms ?? fw : fw;
  if (spoken != null && spoken > SLOW_FIRST_WORD_MS) {
    return {
      tone: "danger",
      title: `Too slow for a good spoken reply: first word in ${ms(spoken)}`,
      help: r.advice ?? (fast ? "A faster fast model, or a provider nearer to you, would help." : "A fast model in front of this one, or a provider nearer to you, would help."),
      firstWordMs: spoken,
    };
  }
  if (main.streaming.state === "no") {
    return {
      tone: "info",
      title: "Works, but without streaming",
      help: "Ghost starts speaking only once the whole reply has come. A provider that streams would start sooner.",
      firstWordMs: spoken,
    };
  }
  if (fast && !fast.reachable) {
    return { tone: "info", title: "Works through the main model; the fast model didn’t answer", help: sentence(fast.error ?? "Check the fast model’s name."), firstWordMs: spoken };
  }
  return { tone: "ok", title: spoken != null ? `Ready to talk: first word in ${ms(spoken)}` : "Ready to talk", help: null, firstWordMs: spoken };
}
