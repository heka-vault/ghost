/**
 * Owner: design. One synthetic day of Ghost's work, as a single event log, for the concept
 * prototypes (docs/design/concepts-2026-11.md). Both concepts draw `stateAt(t)`: what was
 * happening at second `t` of the day. Because the screen is a function of time over one log,
 * pulling time back is a true replay, not a separate history view.
 *
 * Everything here is invented: agents, apps, words. Deterministic (a seeded generator), so a
 * screenshot of 10:41 looks the same every time.
 */

export const H = 3600;
export const M = 60;
/** The moment the scripted sequence starts: "now" when the page opens. */
export const LIVE = 14 * H + 52 * M;
/** Where the day's axis begins. */
export const DAY_START = 7 * H;

export type AgentId = "ghost" | "accounting" | "legal" | "release" | "research" | "inbox";
export type AppId = "github" | "linear" | "calendar" | "mail" | "books" | "files" | "ghost";

export interface Agent {
  id: AgentId;
  name: string;
  job: string;
  /** Index into the design system's agent hues (`--hue-N-fg`). */
  hue: number;
  /** Orbit: radius as a fraction of the field, starting angle, turns per hour. */
  orbit: number;
  phase0: number;
  speed: number;
}

export const AGENTS: Agent[] = [
  { id: "ghost", name: "Ghost", job: "Everything, at the desk", hue: 0, orbit: 0, phase0: 0, speed: 0 },
  { id: "accounting", name: "Accounting", job: "Books, refunds, receipts", hue: 1, orbit: 0.42, phase0: 0.3, speed: 0.31 },
  { id: "legal", name: "Legal", job: "Contracts and renewals", hue: 2, orbit: 0.5, phase0: 2.1, speed: 0.22 },
  { id: "release", name: "Release", job: "Builds and releases", hue: 3, orbit: 0.38, phase0: 4.0, speed: 0.4 },
  { id: "research", name: "Research", job: "What the market shipped", hue: 4, orbit: 0.55, phase0: 5.2, speed: 0.18 },
  { id: "inbox", name: "Inbox", job: "Mail that needs a reply", hue: 5, orbit: 0.46, phase0: 1.2, speed: 0.27 },
];
export const agentOf = (id: AgentId) => AGENTS.find((a) => a.id === id)!;

export interface App {
  id: AppId;
  name: string;
  /** Angle on the outer ring, radians, 0 = right, clockwise (screen y down). */
  angle: number;
}

export const APPS: App[] = [
  { id: "github", name: "GitHub", angle: -2.35 },
  { id: "linear", name: "Linear", angle: -1.57 },
  { id: "calendar", name: "Calendar", angle: -0.78 },
  { id: "mail", name: "Mail", angle: 0.2 },
  { id: "books", name: "Books", angle: 2.95 },
  { id: "files", name: "Files", angle: 2.5 },
  { id: "ghost", name: "Ghost", angle: -3.0 },
];
export const appOf = (id: AppId) => APPS.find((a) => a.id === id)!;

export interface Call {
  id: number;
  t: number;
  dur: number;
  agent: AgentId;
  app: AppId;
  tool: string;
  read: boolean;
  /** Items returned, when the tool lists something. */
  items: number;
  bytes: number;
  /** In a run with nobody there (false: in a session). */
  run: boolean;
}

export type Who = "you" | AgentId;
export interface Utter {
  id: string;
  t: number;
  dur: number;
  who: Who;
  text: string;
}

export interface Run {
  id: string;
  t: number;
  dur: number;
  agent: AgentId;
  schedule: string;
  answer: string;
}

export interface Ask {
  id: string;
  t: number;
  kind: "decision" | "question";
  agent: AgentId;
  text: string;
  choices: string[];
  /** Answered earlier today, in the fixture. Live answers come from the engine. */
  answered?: { at: number; choice: string; how: "voice" | "desk" };
}

export interface Change {
  t: number;
  agent: AgentId;
  text: string;
}

export interface Span {
  t: number;
  dur: number;
}

export interface Day {
  calls: Call[];
  utters: Utter[];
  runs: Run[];
  asks: Ask[];
  changes: Change[];
  thinks: Span[];
  sessions: (Span & { agent: AgentId })[];
  /** Calls per minute since DAY_START, for the axes. */
  perMinute: number[];
}

function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const TOOLS: Record<AppId, { read: string[]; change: string[] }> = {
  github: { read: ["list_issues", "get_issue", "list_workflow_runs", "list_releases", "get_pull_request", "search_code"], change: ["add_labels", "close_issue", "create_comment"] },
  linear: { read: ["list_issues", "get_project", "search"], change: ["update_issue"] },
  calendar: { read: ["list_events", "free_busy"], change: ["create_event"] },
  mail: { read: ["search_threads", "get_thread", "list_unread"], change: ["draft_reply"] },
  books: { read: ["list_transactions", "get_invoice", "list_refunds", "balance"], change: ["categorize"] },
  files: { read: ["read_file", "list_dir", "search"], change: ["write_output"] },
  ghost: { read: ["waiting_list", "recall", "schedules_list", "status"], change: ["notify", "resolve"] },
};

/** Build the day. */
export function buildDay(): Day {
  const r = rng(1112);
  const calls: Call[] = [];
  const utters: Utter[] = [];
  const runs: Run[] = [];
  const asks: Ask[] = [];
  const changes: Change[] = [];
  const thinks: Span[] = [];
  const sessions: (Span & { agent: AgentId })[] = [];
  let id = 0;
  const pick = <T,>(xs: T[]) => xs[Math.floor(r() * xs.length)];

  const call = (t: number, agent: AgentId, app: AppId, run: boolean, read = true, tool?: string, big = 1) => {
    const items = read ? Math.floor(r() * 30 * big) + 1 : 0;
    const bytes = read ? Math.floor((600 + items * (300 + r() * 900)) * big) : 300 + Math.floor(r() * 400);
    const dur = 0.35 + r() * 1.1 + (bytes / 40000) * 0.6;
    calls.push({ id: id++, t, dur, agent, app, tool: tool ?? pick(read ? TOOLS[app].read : TOOLS[app].change), read, items, bytes, run });
    return dur;
  };

  /** A run's calls; `burst` > 1 sends them in parallel waves, as a big job does. */
  const doRun = (rid: string, t: number, dur: number, agent: AgentId, schedule: string, answer: string, apps: AppId[], n: number, big = 1, burst = 1) => {
    runs.push({ id: rid, t, dur, agent, schedule, answer });
    const waves = Math.ceil(n / burst);
    for (let i = 0; i < n; i++) {
      const wave = Math.floor(i / burst);
      const at = t + 1 + (wave / waves) * (dur - 4) + r() * 0.5;
      call(at, agent, pick(apps), true, true, undefined, big);
    }
  };

  // Release checks the build every hour.
  for (let h = 8; h <= 14; h++) {
    const failed = h === 12;
    doRun(`release-${h}`, h * H + 4, 22, "release", "Every hour", failed ? "CI failed on main: test (ubuntu-latest)." : h === 14 ? "4.11.3 is out, every check green." : "Main is green.", ["github"], 3 + Math.floor(r() * 3));
  }
  doRun("accounting-9", 9 * H, 70, "accounting", "Weekdays at 9", "Two refunds over $500 are still pending.", ["books", "mail"], 9, 1.4);
  doRun("inbox-831", 8 * H + 31 * M, 40, "inbox", "When mail arrives", "Two replies drafted; one needs you.", ["mail"], 6);
  doRun("inbox-1120", 11 * H + 20 * M, 35, "inbox", "When mail arrives", "Nothing needs a reply.", ["mail"], 4);
  doRun("inbox-1247", 12 * H + 47 * M, 38, "inbox", "When mail arrives", "The lease renewal came in; passed to Legal.", ["mail", "files"], 5);
  doRun("legal-1015", 10 * H + 15 * M, 64, "legal", "Mondays and Thursdays", "No renewals due this month.", ["files", "mail"], 12, 1.6);
  doRun("legal-1250", 12 * H + 50 * M, 52, "legal", "Asked by Inbox", "The lease renews on 1 December at the same rent.", ["files"], 8, 2);

  // The big one, in the scripted sequence: a scheduled scan that reads a lot, in the background.
  doRun("research-live", LIVE + 16, 18, "research", "Thursdays at 2:50", "Weekly scan done: three products shipped voice modes; notes in out/scan-11-12.md.", ["mail", "files", "github", "linear"], 36, 2.2, 6);
  changes.push({ t: LIVE + 33.2, agent: "research", text: "Wrote out/scan-11-12.md" });
  call(LIVE + 33, "research", "files", false, false, "write_output");

  // Sessions earlier in the day.
  const session = (t: number, agent: AgentId, turns: [string, string, AppId[]][]) => {
    let at = t;
    for (const [you, reply, apps] of turns) {
      const yd = 1.2 + you.length / 16;
      utters.push({ id: `u${id++}`, t: at, dur: yd, who: "you", text: you });
      at += yd + 0.3;
      const th = 0.6 + apps.length * 0.5;
      thinks.push({ t: at, dur: th });
      apps.forEach((a, i) => call(at + 0.15 + i * 0.35, agent, a, false));
      at += th;
      const rd = 1 + reply.length / 15;
      utters.push({ id: `u${id++}`, t: at, dur: rd, who: agent, text: reply });
      at += rd + 1.5;
    }
    sessions.push({ t, dur: at - t + 4, agent });
    return at;
  };

  session(9 * H + 12 * M, "ghost", [
    ["Ghost, what's on today?", "Three meetings, the first at ten. Accounting found two refunds still pending, and the build is green.", ["calendar", "ghost", "github"]],
    ["Move the ten o'clock to eleven.", "Moved. Dana has it.", ["calendar"]],
  ]);
  changes.push({ t: 9 * H + 12 * M + 22, agent: "ghost", text: "Moved Design review to 11:00" });

  asks.push({ id: "q-1002", t: 10 * H + 2 * M, kind: "question", agent: "accounting", text: "Should October's software spend go under R&D or Operations?", choices: ["R&D", "Operations"], answered: { at: 10 * H + 41 * M + 9, choice: "R&D", how: "voice" } });
  session(10 * H + 41 * M, "accounting", [["R&D. And file the receipts.", "Done: under R&D, nine receipts filed.", ["books", "files"]]]);
  changes.push({ t: 10 * H + 41 * M + 14, agent: "accounting", text: "Categorized 9 transactions as R&D" });

  asks.push({ id: "d-1306", t: 13 * H + 6 * M + 10, kind: "decision", agent: "ghost", text: "Add priority-high to ghost-mock/app#212?", choices: ["Yes", "No"], answered: { at: 13 * H + 6 * M + 19, choice: "Yes", how: "desk" } });
  session(13 * H + 5 * M, "ghost", [
    ["Ghost, why did CI fail at noon?", "A flaky test on ubuntu; the rerun passed. Issue 212 tracks it. Mark it high?", ["github", "github", "linear"]],
  ]);
  changes.push({ t: 13 * H + 6 * M + 20, agent: "ghost", text: "Labelled ghost-mock/app#212 priority-high" });

  // The scripted sequence, from LIVE.
  const wake = LIVE + 5;
  utters.push({ id: "live-you", t: wake, dur: 4.4, who: "you", text: "Ghost, what's waiting on me, and did the release go out?" });
  thinks.push({ t: wake + 4.6, dur: 3.6 });
  const live: [number, AppId, string, number][] = [
    [0.2, "github", "list_releases", 1],
    [0.5, "github", "list_workflow_runs", 2.5],
    [0.9, "ghost", "waiting_list", 1],
    [1.4, "linear", "list_issues", 1.5],
    [1.8, "github", "get_issue", 1],
  ];
  for (const [o, app, tool, big] of live) call(wake + 4.6 + o, "ghost", app, false, true, tool, big);
  utters.push({ id: "live-ghost", t: wake + 8.4, dur: 9.4, who: "ghost", text: "Two things. 4.11.3 went out at ten past two and every check is green. Issue 151 looks fixed by it: want me to close it?" });
  asks.push({ id: "d-live", t: wake + 17.2, kind: "decision", agent: "ghost", text: "Close ghost-mock/app#151 as fixed in 4.11.3?", choices: ["Yes", "No"] });
  sessions.push({ t: wake, dur: 27, agent: "ghost" });
  asks.push({ id: "q-live", t: LIVE + 37, kind: "question", agent: "accounting", text: "Refund the duplicate $86 charge from the business account or the personal one?", choices: ["Business", "Personal", "Not now"] });
  call(LIVE + 36.4, "accounting", "books", true, true, "list_refunds");

  calls.sort((a, b) => a.t - b.t);
  utters.sort((a, b) => a.t - b.t);
  const perMinute = new Array(Math.ceil((LIVE + 10 * M - DAY_START) / M)).fill(0);
  for (const c of calls) perMinute[Math.floor((c.t - DAY_START) / M)] += 1;
  return { calls, utters, runs, asks, changes, thinks, sessions, perMinute };
}

export type Phase = "passive" | "you" | "thinking" | "speaking" | "listening";

export interface Answer {
  at: number;
  choice: string;
  how: "voice" | "desk";
}

export interface State {
  t: number;
  phase: Phase;
  /** Voice level 0..1 (the operator's while they speak, Ghost's while it speaks). */
  level: number;
  utter: Utter | null;
  /** How much of `utter` has been said, 0..1. */
  said: number;
  session: (Span & { agent: AgentId }) | null;
  active: Call[];
  running: Run[];
  /** Asks open at `t`, oldest first. */
  waiting: Ask[];
  /** Calls in the last `window` seconds, for trails. */
  recent: Call[];
}

const within = (s: Span, t: number) => t >= s.t && t < s.t + s.dur;

function hash(n: number) {
  const x = Math.sin(n * 127.1) * 43758.5453;
  return x - Math.floor(x);
}

/** A believable speech envelope: syllables, words, breath. Deterministic in t. */
export function levelAt(u: Utter, t: number): number {
  const local = t - u.t;
  if (local < 0 || local > u.dur) return 0;
  const env = Math.min(1, local / 0.12) * Math.min(1, (u.dur - local) / 0.25);
  const syll = 0.5 + 0.5 * Math.sin(local * 2 * Math.PI * 4.1 + hash(u.t) * 6);
  const word = 0.55 + 0.45 * Math.sin(local * 2 * Math.PI * 0.9 + 1.3);
  const n = hash(Math.floor(t * 24) + u.t);
  const gap = hash(Math.floor(local * 2.5) + 9) < 0.12 ? 0.15 : 1;
  return Math.max(0, Math.min(1, env * gap * (0.18 + 0.82 * syll * word * (0.6 + 0.4 * n))));
}

export function answerOf(a: Ask, answers: Map<string, Answer>): Answer | undefined {
  return a.answered ?? answers.get(a.id);
}

export function stateAt(day: Day, t: number, answers: Map<string, Answer>, window = 8): State {
  const utter = day.utters.find((u) => within(u, t)) ?? null;
  const session = day.sessions.find((s) => within(s, t)) ?? null;
  const thinking = day.thinks.some((s) => within(s, t));
  const phase: Phase = utter ? (utter.who === "you" ? "you" : "speaking") : thinking ? "thinking" : session ? "listening" : "passive";
  const active: Call[] = [];
  const recent: Call[] = [];
  for (const c of day.calls) {
    if (c.t > t) break;
    if (t < c.t + c.dur) active.push(c);
    else if (t - (c.t + c.dur) < window) recent.push(c);
  }
  const running = day.runs.filter((r) => within(r, t));
  const waiting = day.asks.filter((a) => {
    if (a.t > t) return false;
    const ans = answerOf(a, answers);
    return !ans || ans.at > t;
  });
  return {
    t,
    phase,
    level: utter ? levelAt(utter, t) : 0,
    utter,
    said: utter ? Math.min(1, (t - utter.t) / utter.dur) : 0,
    session,
    active,
    running,
    waiting,
    recent,
  };
}

/** Clock text, "14:52" or "14:52:07". */
export function clock(t: number, seconds = false): string {
  const s = Math.max(0, Math.floor(t));
  const hh = String(Math.floor(s / H)).padStart(2, "0");
  const mm = String(Math.floor((s % H) / M)).padStart(2, "0");
  const ss = String(s % M).padStart(2, "0");
  return seconds ? `${hh}:${mm}:${ss}` : `${hh}:${mm}`;
}

/** Bytes as "4.1 kB". */
export function size(b: number): string {
  if (b < 1000) return `${b} B`;
  if (b < 1e6) return `${(b / 1000).toFixed(1)} kB`;
  return `${(b / 1e6).toFixed(2)} MB`;
}
