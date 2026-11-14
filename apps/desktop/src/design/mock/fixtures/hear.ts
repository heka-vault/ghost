/**
 * Owner: hear. The Hearing test's mock data (ADR 0042): a short take list with a few takes in
 * blocks A, B, F (across the room), G (music) and H (silent), the operator's sessions, and
 * score runs with a report built the way the daemon's is (tables with verdicts, the pass rule
 * against a baseline). What Ghost heard of a take is made up per take by the mock daemon.
 */
import type { HearPlan, HearReport, HearRow, HearRunInfo, HearSection, HearTake } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

export const HEAR_PLAN: HearPlan = {
  version: 1,
  about: "The hear test set: what the operator says, block by block. ' ... ' in a line is a pause of about a second.",
  blocks: [
    {
      block: "A",
      condition: "desk",
      title: "At the desk, normal voice",
      instruction: "Sit where you usually sit and speak the way you talk to Ghost on an ordinary day. Say each line once, as a request.",
      takes: [
        { id: "A01", kind: "speech", text: "What needs my sign-off today?" },
        { id: "A02", kind: "speech", text: "Read me issue four oh three.", tags: ["numbers"] },
        { id: "A03", kind: "speech", text: "Close issue one forty two in the app repo.", tags: ["numbers"] },
        { id: "A04", kind: "speech", text: "Any new issues on heka vault ghost since yesterday?", tags: ["terms"], terms: ["heka vault", "Ghost"] },
        { id: "A05", kind: "speech", text: "Search Mobbin for onboarding screens with a progress bar.", tags: ["terms"], terms: ["Mobbin"] },
      ],
    },
    {
      block: "B",
      condition: "desk-pause",
      title: "At the desk, a pause mid-sentence",
      instruction: "Where the line shows a pause, stop for about a second, the way you do when you’re thinking of the next word, then carry on. Keep holding the key through the pause.",
      takes: [
        { id: "B01", kind: "speech", text: "Read me issue four one three ... from the acme research project.", tags: ["pause", "numbers", "terms"], terms: ["acme"] },
        { id: "B02", kind: "speech", text: "What’s waiting for triage ... in the app repo?", tags: ["pause"] },
        { id: "B03", kind: "speech", text: "I noticed that when I pause ... in the middle of a sentence ... you cut me off.", tags: ["pause"] },
      ],
    },
    {
      block: "F",
      condition: "across-room",
      title: "Across the room, 3 m or more",
      instruction: "Press once, then walk to a spot 3 m or more from the mic and face it. The countdown is longer so you have time; count it in your head if you can’t read the screen. Normal voice, no shouting.",
      away: true,
      takes: [
        { id: "F01", kind: "speech", text: "What needs my sign-off today?", tags: ["far"] },
        { id: "F02", kind: "reply", text: "Yes.", tags: ["far"] },
        { id: "F03", kind: "speech", text: "What’s on my plate ... for this afternoon?", tags: ["far", "pause"] },
      ],
    },
    {
      block: "G",
      condition: "music",
      title: "At the desk, with music playing",
      instruction: "Start some music yourself on the desk speakers, at the level you usually listen at (the recorder plays nothing). Stay at the desk, normal voice. The last take is silent: keep the music on and say nothing. Stop the music when the block ends.",
      music: true,
      takes: [
        { id: "G01", kind: "speech", text: "Close issue one forty two.", tags: ["music", "numbers"] },
        { id: "G02", kind: "reply", text: "Stop.", tags: ["music"] },
        { id: "G03", kind: "silence", text: "", say: "(say nothing; music on)", seconds: 6, tags: ["music"] },
      ],
    },
    {
      block: "H",
      condition: "desk-silence",
      title: "At the desk, saying nothing",
      instruction: "Music off. Say nothing in these takes. Each line says what to do instead: sit still, type, or move about the way you do at the desk. Any word Ghost hears in them is made up.",
      takes: [
        { id: "H01", kind: "silence", text: "", say: "(sit still, say nothing)", seconds: 6 },
        { id: "H02", kind: "silence", text: "", say: "(type on the keyboard, say nothing)", seconds: 6, tags: ["typing"] },
        { id: "H03", kind: "silence", text: "", say: "(move about: the chair, a cup, a drawer; say nothing)", seconds: 6, tags: ["moving"] },
      ],
    },
  ],
};

export interface MockHearSession {
  name: string;
  updated: string | null;
  takes: HearTake[];
}

export interface MockHearRun {
  info: HearRunInfo;
}

export interface HearFixture {
  plan: HearPlan;
  sessions: MockHearSession[];
  runs: MockHearRun[];
  /** The raw array under ghost.mic, by node name (null: none, takes have ghost.mic only). */
  rawDevice: string | null;
}

/** A recorded take as its manifest line says it: the line asked for, never what was heard. */
export function recordedTake(plan: HearPlan, id: string, patch: Partial<HearTake> = {}): HearTake {
  const block = plan.blocks.find((b) => b.takes.some((t) => t.id === id))!;
  const t = block.takes.find((x) => x.id === id)!;
  const silent = t.kind === "silence";
  const seconds = silent ? (t.seconds ?? 6) : 2.2 + (t.text ?? "").split(/\s+/).length * 0.32;
  return {
    id,
    block: block.block,
    kind: t.kind,
    text: t.text ?? "",
    state: "recorded",
    seconds: Math.round(seconds * 10) / 10,
    peak: silent ? -41 : -9 - (id.charCodeAt(2) % 5),
    speech_start: silent ? null : 0.52,
    speech_end: silent ? null : Math.round((seconds - 0.6) * 10) / 10,
    ending: silent ? "timed" : block.away ? "quiet" : "hold",
    warnings: [],
    raw: true,
    time: ago(60 * 20),
    ...patch,
  };
}

const ids = (plan: HearPlan) => plan.blocks.flatMap((b) => b.takes.map((t) => t.id));

function run(id: string, session: string, channel: "aec" | "raw", minutes: number, wer: [number, number, number], note: string, takes = 17): MockHearRun {
  return {
    info: {
      id,
      session,
      channel,
      created: new Date(Date.now() - minutes * 60_000).toISOString().slice(0, 16).replace("T", " "),
      commit: channel === "raw" ? "cc2808c" : "0e534db",
      takes,
      wer: { stage: wer[0], turn: wer[1], file: wer[2] },
      note,
    },
  };
}

export default function hear(s: Scenario): HearFixture {
  const plan = structuredClone(HEAR_PLAN);
  if (s.data === "empty") return { plan, sessions: [], runs: [], rawDevice: null };
  const all = ids(plan);
  const yesterday: MockHearSession = {
    name: "2026-11-05",
    updated: ago(60 * 20),
    takes: all.map((id) =>
      id === "A04"
        ? recordedTake(plan, id, { warnings: ["cut_off"], speech_end: 4.9, seconds: 4.9 })
        : id === "B02"
          ? recordedTake(plan, id, { warnings: ["started_early", "too_quiet"], speech_start: 0, peak: -38 })
          : id === "F02"
            ? recordedTake(plan, id, { state: "skipped", seconds: null, peak: null, speech_start: null, speech_end: null, ending: null, raw: false })
            : recordedTake(plan, id),
    ),
  };
  const half: MockHearSession = {
    name: "2026-11-06",
    updated: ago(25),
    takes: all.slice(0, 6).map((id) => recordedTake(plan, id, { time: ago(25), raw: false, ...(id === "A03" ? { warnings: ["missed_end"] } : {}) })),
  };
  const sessions = [half, yesterday];
  if (s.data === "busy") sessions.push({ name: "2026-10-31", updated: ago(60 * 24 * 6), takes: all.map((id) => recordedTake(plan, id, { raw: false, time: ago(60 * 24 * 6) })) });
  const runs = [
    run("2026-11-06-0912", "2026-11-05", "raw", 50, [0.094, 0.061, 0.049], "Raw array, same session: is the canceller costing words?"),
    run("2026-11-05-2140", "2026-11-05", "aec", 60 * 12, [0.081, 0.052, 0.044], "Main before #52: the baseline."),
  ];
  if (s.data === "busy") runs.push(run("2026-11-01-1015", "2026-10-31", "aec", 60 * 24 * 5, [0.112, 0.07, 0.051], ""));
  return { plan, sessions, runs, rawDevice: "alsa_input.usb-SEEED_ReSpeaker_XVF3800-00.multichannel-input" };
}

// ------------------------------------------------------------------ a run's report

const row = (cells: string[], verdict: HearRow["verdict"] = null, take: string | null = null): HearRow => ({ cells, verdict, take });

/** A report as the daemon renders one: the summary, by block, the takes to listen to; against a baseline, the comparison and the pass rule. */
export function reportFor(info: HearRunInfo, baseline: HearRunInfo | null): HearReport {
  const w = info.wer;
  const p = (x: number | null | undefined) => (x == null ? "-" : `${(x * 100).toFixed(1)}%`);
  const sections: HearSection[] = [
    {
      level: 2,
      title: "Summary",
      parts: [
        {
          kind: "table",
          columns: ["", "stage", "turn", "file"],
          rows: [
            row(["word error rate (requests and replies)", p(w.stage), p(w.turn), p(w.file)]),
            row(["number errors", "1 of 12", "0 of 12", "0 of 12"]),
            row(["terms heard", "4 of 6", "5 of 6", "5 of 6"]),
            row(["empty reads (requests and replies)", "0 of 11", "0 of 11", "0 of 11"]),
            row(["silent takes with words", "1 of 4", "1 of 4", "2 of 4"]),
            row(["takes split in pieces", "5 of 11", "0 of 11", "0 of 11"]),
            row(["read at the close, p95", "0.17 s", "0.23 s", "0.27 s"]),
          ],
        },
      ],
    },
    {
      level: 2,
      title: "By block",
      parts: [
        {
          kind: "table",
          columns: ["Block", "Condition", "Takes", "stage", "turn", "file"],
          rows: [
            row(["A", "desk", "5", "6.1%", "3.0%", "3.0%"]),
            row(["B", "desk-pause", "3", "11.8%", "5.9%", "2.9%"]),
            row(["F", "across-room", "2", "14.3%", "9.5%", "9.5%"]),
            row(["G", "music", "3", "9.1%", "9.1%", "4.5%"]),
            row(["H", "desk-silence", "3", "1 made up", "1 made up", "2 made up"]),
          ],
        },
        { kind: "text", text: "A speech block’s cell is its word error rate; a silent block’s is the words made up." },
      ],
    },
  ];
  if (baseline) {
    const b = baseline.wer;
    const delta = (a: number | null | undefined, c: number | null | undefined) => (a == null || c == null ? "-" : `${a - c < 0 ? "−" : "+"}${Math.abs((a - c) * 100).toFixed(1)}%`);
    const worse = (w.stage ?? 0) > (b.stage ?? 0) + 0.01;
    sections.push({
      level: 2,
      title: `\`stage\` against the baseline (${baseline.id})`,
      parts: [
        {
          kind: "facts",
          facts: [
            { name: "baseline", value: `${baseline.id} · ${baseline.session} · ${baseline.channel}` },
            { name: "paired takes", value: "15" },
          ],
        },
        {
          kind: "table",
          columns: ["Measure", "Takes", "Before", "After", "Change", "95% interval", "Verdict"],
          rows: [
            row(["word error rate", "11", p(b.stage), p(w.stage), delta(w.stage, b.stage), "−4.1% to +1.2%", "within noise"], "same"),
            row(["number errors", "11", "8.3%", "0.0%", "−8.3%", "−16.7% to −1.0%", "better"], "better"),
            row(["term errors", "11", "16.7%", "33.3%", "+16.7%", "+2.1% to +33.3%", "worse"], worse ? "worse" : "same"),
            row(["empty reads", "11", "0.0%", "0.0%", "0.0%", "0.0% to 0.0%", "within noise"], "same"),
            row(["silent takes with words", "4", "0.0%", "25.0%", "+25.0%", "0.0% to +75.0%", "within noise"], "same"),
          ],
        },
        { kind: "text", text: "Per take, fewer errors after / the same / more: 3 / 10 / 2 (damage: 2 of 15)." },
        { kind: "pass", pass: !worse, text: worse ? "Fails: term errors are worse beyond noise." : "Passes: no measure is worse beyond noise, and the close’s p95 grew by 0.02 s (budget 0.15 s)." },
      ],
    });
    sections.push({
      level: 3,
      title: "Read at the close",
      parts: [{ kind: "text", text: "p95: 0.15 s against 0.17 s, inside the budget of `--close-budget-ms 150`." }],
    });
  }
  sections.push({
    level: 2,
    title: "The takes to listen to",
    parts: [
      { kind: "text", text: "The most errors in `stage`. Play a take to hear what Ghost was given." },
      {
        kind: "table",
        columns: ["Take", "Source", "Words said", "Segments", "stage", "turn", "file"],
        rows: [
          row(["A04", "take", "9", "2", "3", "2", "2"], null, "A04|take"),
          row(["B03", "take", "15", "3", "3", "1", "0"], null, "B03|take"),
          row(["F03", "take", "8", "2", "2", "1", "1"], null, "F03|take"),
          row(["A05", "take", "9", "1", "1", "1", "1"], null, "A05|take"),
        ],
      },
    ],
  });
  return {
    meta: [
      { name: "channel", value: info.channel === "raw" ? "raw (the array under ghost.mic)" : "aec (ghost.mic)" },
      { name: "commit", value: info.commit },
      { name: "date", value: info.created },
      { name: "session", value: info.session },
      { name: "speech to text", value: "nemo-parakeet-tdt-0.6b-v3 (fp32), 4 threads" },
      { name: "vad", value: "confidence 0.7, start 0.2 s, stop 0.2 s, min volume 0.6" },
      { name: "takes", value: String(info.takes) },
    ],
    intro: "`stage` is what the speech-in stage hands the aggregator; `turn` is the turn’s audio read once at the close; `file` is the whole take read once with no VAD. Fillers are left out of every count; numbers are compared as digits.",
    sections,
    pass: baseline ? !((w.stage ?? 0) > (baseline.wer.stage ?? 0) + 0.01) : null,
  };
}
