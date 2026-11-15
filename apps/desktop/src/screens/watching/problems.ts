/**
 * Owner: watching. What can't run, said for a newcomer (the owner's review, 2026-11-14): one
 * plain sentence per problem with its fix where there is one, the daemon's own line and file
 * kept for Details.
 *
 * The daemon reports these as formatted strings today (`SchedulesView.problems`,
 * `WatchingView.problems`: `ghost_core::schedules::jobs::problems`, `ghost_core::folder`,
 * `ghost_core::watches::parse_watches`, `ghost_core::hooks`), not typed fields. They are Ghost's
 * own words about the operator's own files, never an app's text, so this maps the known shapes
 * of those lines deterministically: a schedule whose agent is retired or gone, a watch whose app
 * isn't connected or whose agent is retired, a watch or webhook whose declaration doesn't read,
 * a webhook that is off, a file that can't be read. Anything else is “Something in your setup
 * can't be read” with its Details. Every name taken from a line is bounded; a sentence never
 * carries more of the line than a name. Pure, so it is tested (test/on-its-own.test.ts).
 */

export interface Problem {
  /** The plain sentence: what can't run, and why, in Ghost's words. */
  sentence: string;
  /** Where it is fixed, when there is one place: a route and the words of its link. */
  fix: { label: string; path: string } | null;
  /** The daemon's line as it came, for Details. */
  detail: string;
  /** The file it is about, for Details, when the line names one. */
  file: string | null;
}

const NAME_MAX = 48;
/** A name out of a line: trimmed, quotes off, bounded. */
const nameOf = (s: string) => {
  const t = s.trim().replace(/^["“'`]|["”'`]$/g, "");
  return t.length > NAME_MAX ? `${t.slice(0, NAME_MAX - 1)}…` : t;
};

const FILE = /(^|\s)(\S*\/)?(watches\.json|hooks\.json|AGENTS\.md)\b/;

export interface ProblemContext {
  appTitle: (name: string) => string;
  agentName: (slug: string) => string;
}

export function problemOf(raw: string, c: ProblemContext): Problem {
  const line = raw.trim();
  const fileHit = FILE.exec(line);
  const file = fileHit ? `${fileHit[2] ?? ""}${fileHit[3]}` : null;
  const base = { detail: line, file };
  let m: RegExpExecArray | null;

  // A schedule whose agent is retired or isn't there (`jobs::problems`).
  if ((m = /^The schedule (.+?) is for (\S+), which (is retired|is not an agent Ghost has); it will not run$/.exec(line))) {
    const who = c.agentName(nameOf(m[2]));
    return {
      ...base,
      sentence: m[3] === "is retired" ? `The ${nameOf(m[1])} schedule can’t run: ${who} is retired.` : `The ${nameOf(m[1])} schedule can’t run: there is no agent called ${who}.`,
      fix: { label: "See your agents", path: "/settings/agents" },
    };
  }

  // A webhook the daemon turned off (`hooks.rs`: “hook <name> is off: <why>”).
  if ((m = /^hook (\S+) is off: (its password \S+ is not set|.+)$/.exec(line))) {
    const pw = /^its password (\S+) is not set$/.exec(m[2]);
    return pw
      ? { ...base, sentence: `The ${nameOf(m[1])} webhook is off: its password isn’t set.`, fix: { label: "Open Passwords", path: "/settings/passwords" } }
      : { ...base, sentence: `The ${nameOf(m[1])} webhook is off: its recorded deliveries don’t check out.`, fix: null };
  }

  // A line about one entry of a file: “<file>: [watch |hook ]<name>: <what>”.
  if (file && (m = /:\s+(?:(watch|hook)\s+)?([^\s:]+):\s+(.+)$/.exec(line))) {
    const kind = m[1] ?? (file.endsWith("hooks.json") ? "hook" : "watch");
    const what = kind === "hook" ? "webhook" : "watch";
    const name = nameOf(m[2]);
    const rest = m[3];
    const app = /\bapp\s+["“]?([A-Za-z0-9_-]{1,64})["”]?\s+(?:is not|isn’t|isn't) connected/.exec(rest);
    if (app) {
      // An app with no catalog title is named by its own name, first letter up (“Quickbooks”).
      const named = c.appTitle(app[1]);
      const title = named === app[1] ? named.charAt(0).toUpperCase() + named.slice(1) : named;
      return { ...base, sentence: `The ${name} ${what} can’t run: ${title} isn’t connected.`, fix: { label: `Connect ${title}`, path: "/apps" } };
    }
    const agent = /\bagent\s+["“]?([a-z0-9-]{1,64})["”]?\b.*\b(retired|not an agent|isn’t an agent|doesn't have|doesn’t have)/i.exec(rest);
    if (agent) {
      return { ...base, sentence: `The ${name} ${what} can’t act: ${c.agentName(agent[1])} ${/retired/i.test(agent[2]) ? "is retired" : "isn’t one of your agents"}.`, fix: { label: "See your agents", path: "/settings/agents" } };
    }
    return { ...base, sentence: `The ${name} ${what} is off: Ghost can’t read how it is set up.`, fix: null };
  }

  // A whole file that doesn't read, or isn't the shape Ghost reads.
  if (file) {
    const which = file.endsWith("watches.json") ? "Your watches file" : file.endsWith("hooks.json") ? "Your webhooks file" : "Your rules file";
    return { ...base, sentence: `${which} can’t be read, so nothing in it runs.`, fix: null };
  }

  return { ...base, sentence: "Something in your setup can’t be read.", fix: null };
}
