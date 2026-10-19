/**
 * Owner: conversations. Lines with their persona, memory, pinned notes, transcript
 * history and compactions; plus the Kokoro voice list. Extend freely.
 */
import type { CompactionView, TurnView, VoiceInfo } from "@ghost/client-ts";

import type { Scope, ThreadDetail } from "../../../screens/conversations/threads";
import { ago, type Scenario, seeded } from "../scenarios";
import { SESSION_ID } from "./session";

const HOME = "~/.local/share/ghost/conversations";

export interface LineRecord {
  detail: ThreadDetail;
  turns: TurnView[];
  compactions: CompactionView[];
}

interface Seed {
  slug: string;
  name: string;
  aliases: string[];
  voice: string;
  speed: number;
  persona: string;
  memory: string;
  notes: string[];
  createdDaysAgo: number;
  lastActiveMin: number | null;
  turns: [TurnView["role"], string][];
  compactions: number;
  /** What the line may touch, per ability (ADR 0019). Absent: unscoped, it sees everything. */
  scope?: Scope;
  /** Settled this many days ago (ADR 0019): out of voice routing and the active list, still readable. */
  settledDaysAgo?: number;
  /**
   * Each version's summary, oldest first, one per compaction; the newest is `memory`.
   * Aliases never repeat the line's own name or slug: ghost_core matches on (name, *aliases).
   */
  history?: string[];
}

const GHOST: Seed = {
  slug: "ghost",
  name: "Ghost",
  aliases: ["main line", "primary"],
  voice: "af_heart",
  speed: 1.0,
  persona: "Short answers. Tell me what changed and what's waiting on me, then stop talking.",
  memory:
    "Operator triages ghost-mock/app and ghost-mock/infra most mornings around 9.\n" +
    "Prefers issue numbers read as plain numbers (“app 151”), no titles unless asked.\n" +
    "Open: #148 Safari cookie fix is in review with Dana; #151 looks like a duplicate.\n" +
    "Decided: anything labeled wontfix gets closed without a comment.",
  notes: ["Never close issues in ghost-mock/infra without asking.", "Dana owns auth; Priya owns the data pipeline."],
  history: [
    "Operator triages ghost-mock/app most mornings.\nOpen: #140 is waiting on the operator's review before Dana merges.",
    "Operator triages ghost-mock/app and ghost-mock/infra most mornings around 9.\nOpen: #140 reviewed, Dana is merging it.\nClosed #133 as a duplicate of #129.",
    "Operator triages ghost-mock/app and ghost-mock/infra most mornings around 9.\nPrefers issue numbers read as plain numbers (“app 151”), no titles unless asked.\nDecided: duplicates get closed with a link to the older issue, no other comment.",
    "Operator triages ghost-mock/app and ghost-mock/infra most mornings around 9.\nPrefers issue numbers read as plain numbers (“app 151”), no titles unless asked.\nOpen: #148 Safari cookie fix is in review with Dana.\nDecided: duplicates get closed with a link to the older issue.",
    "Operator triages ghost-mock/app and ghost-mock/infra most mornings around 9.\nPrefers issue numbers read as plain numbers (“app 151”), no titles unless asked.\nOpen: #148 Safari cookie fix is in review with Dana; #151 looks like a duplicate.\nDecided: anything labeled wontfix gets closed with a short comment.",
  ],
  createdDaysAgo: 41,
  lastActiveMin: 2,
  turns: [
    ["assistant", "Ghost here."],
    ["user", "What's waiting on me in the app repo?"],
    ["assistant", "Four issues need your sign-off in ghost-mock/app. The oldest is 151, a login loop on Safari after a password reset."],
    ["user", "Isn't that the same as 148?"],
    ["assistant", "It looks like it. 148 is the Safari cookie bug, and its fix is in review."],
  ],
  compactions: 6,
};

const ACCOUNTING: Seed = {
  slug: "accounting",
  name: "Accounting",
  aliases: ["billing", "the books"],
  voice: "bm_george",
  speed: 1.05,
  persona: "You're the accounting desk. Keep me straight on invoices, vendor renewals and the billing bugs in ghost-mock/app. Numbers first, no small talk.",
  memory:
    "Billing bugs live in ghost-mock/app under the billing label; the ledger export is ghost-mock/infra.\n" +
    "Q3 close finished October 9. Two refunds still open with finance (Northwind, Halcyon).\n" +
    "#212 euro rounding: labeled bug, priority high. #208 approved (Dana's fix).",
  notes: ["Stripe renewal is due November 3.", "Halcyon refund: wait for their PO number before approving anything."],
  history: [
    "Billing bugs live in ghost-mock/app under the billing label.\nQ3 close is running, target October 9. Three refunds open with finance (Northwind, Halcyon, Brightline).",
    "Billing bugs live in ghost-mock/app under the billing label; the ledger export is ghost-mock/infra.\nQ3 close finished October 9. Brightline's refund paid October 8; Northwind and Halcyon still open.\n#208 is waiting on Dana's fix.",
  ],
  createdDaysAgo: 18,
  lastActiveMin: 3,
  scope: { github_issues: ["ghost-mock/app", "ghost-mock/infra"] },
  turns: [
    ["system", "transferred from Ghost"],
    ["assistant", "Accounting line."],
    ["user", "What's waiting on me in billing?"],
    ["call", "github_issues.search(label=billing, repo=ghost-mock/app, state=open)"],
    ["assistant", "Three issues: app 212, app 208 and infra 77."],
    ["user", "Label 212 as a bug and high priority."],
    ["call", "change(operation=github_issues.label, target=ghost-mock/app#212)"],
    ["assistant", "Done. 212 now has bug and priority high."],
    ["user", "Approve 208, Dana's fix looks right to me."],
    ["assistant", "Approved app 208 and left your note."],
  ],
  compactions: 3,
};

const LEGAL: Seed = {
  slug: "legal",
  name: "Legal",
  aliases: ["compliance", "counsel"],
  voice: "bf_emma",
  speed: 0.95,
  persona: "Careful and precise. Licenses, data-processing agreements and anything that could be a compliance problem. Quote exactly; never guess at what a contract says.",
  memory:
    "License audit is tracked in ghost-mock/infra #91. Two AGPL tools in the build image are build-time only.\n" +
    "DPA with Northwind renews in January; counsel is Marco.",
  notes: ["The Northwind DPA renews in January.", "Anything touching customer data exports needs-legal-review."],
  createdDaysAgo: 12,
  lastActiveMin: 60 * 26,
  turns: [
    ["assistant", "Legal line."],
    ["user", "Anything new on the license audit?"],
    ["assistant", "Infra 91 flags two packages under AGPL in the build image."],
    ["user", "They don't ship. Comment that and add needs-legal-review."],
    ["assistant", "Commented on infra 91 and added needs-legal-review."],
  ],
  compactions: 1,
};

/** Months of the Legal line (busy): each version resolves something and picks up something new. */
const LEGAL_HISTORY = [
  "License audit opened as ghost-mock/infra #91. Two AGPL tools found in the build image; not yet clear whether they ship.",
  "License audit is ghost-mock/infra #91. Two AGPL tools in the build image; Priya is checking whether they ship.\nNorthwind wants a DPA. Counsel is Jordan; first draft expected at the end of the month.",
  "License audit is ghost-mock/infra #91. The AGPL tools are build-time only since infra #88.\nNorthwind DPA: Jordan handed it to Marco. Redlines due Tuesday.",
  "License audit is ghost-mock/infra #91. Two AGPL tools in the build image are build-time only.\nNorthwind DPA: Marco sent redlines. Open points: retention and the subprocessor list.\nData exports need your sign-off, then Marco's (decided on app #150).",
];

const STANDUP: Seed = {
  slug: "standup",
  name: "Standup",
  aliases: ["daily", "morning update"],
  voice: "am_michael",
  speed: 1.15,
  persona: "Every morning: what moved overnight, what's blocked, who's waiting on me. Ninety seconds, tops.",
  memory: "Standup runs at 9:15. Blocked items get read first.",
  notes: [],
  createdDaysAgo: 7,
  lastActiveMin: 60 * 20,
  turns: [
    ["assistant", "Standup."],
    ["user", "Go."],
    ["assistant", "Overnight: two issues closed, one reopened. Blocked: infra 77, the ledger export."],
  ],
  compactions: 0,
};

/** Scoped to one repository each (ADR 0019): the voice alone says which project is in play. */
const INFRA: Seed = {
  slug: "infra",
  name: "Infra",
  aliases: ["the infra repo", "ops"],
  voice: "am_onyx",
  speed: 1.0,
  persona: "The infra repo only. Deploys, the ledger export, anything paging. Say what broke, what changed, and who's on it.",
  memory: "Infra 77, the ledger export, times out past 10k rows; Priya is on it and it's labeled blocked.\nThe nightly deploy moved to 02:00 after the October 6 outage.",
  notes: ["Never close an infra issue without asking."],
  scope: { github_issues: ["ghost-mock/infra"] },
  createdDaysAgo: 9,
  lastActiveMin: 60 * 5,
  turns: [
    ["assistant", "Infra line."],
    ["user", "What's open on the ledger export?"],
    ["call", "github_issues.search(label=export, state=open)"],
    ["assistant", "One: 77, the timeout past ten thousand rows. Priya has it and it's still blocked."],
    ["user", "Leave it. Anything from last night's deploy?"],
    ["assistant", "Nothing new. The 02:00 deploy went through clean."],
  ],
  compactions: 1,
};

const APP: Seed = {
  slug: "app",
  name: "App",
  aliases: ["the app repo", "frontend"],
  voice: "af_sky",
  speed: 1.05,
  persona: "The app repo only. Bugs first, then what's waiting on my sign-off. Read issue numbers plain.",
  memory: "App 151 is the Safari login loop; likely a duplicate of 148, whose fix is in review with Dana.\nDecided: duplicates get closed with a link to the older issue, no other comment.",
  notes: [],
  scope: { github_issues: ["ghost-mock/app"] },
  createdDaysAgo: 9,
  lastActiveMin: 60 * 2,
  turns: [
    ["assistant", "App line."],
    ["user", "Anything new since yesterday?"],
    ["call", "github_issues.list_awaiting_signoff()"],
    ["assistant", "Two need your sign-off: 151, the Safari login loop, and 162, the CSV export."],
    ["user", "Is 151 the same as 148?"],
    ["assistant", "It looks like it. 148 is the Safari cookie bug, and its fix is in review."],
  ],
  compactions: 1,
};

/** A settled line (ADR 0019): the Q3 close is done, so it left voice routing; everything stays readable. */
const Q3_CLOSE: Seed = {
  slug: "q3-close",
  name: "Q3 close",
  aliases: ["the close", "quarter close"],
  voice: "bf_alice",
  speed: 1.0,
  persona: "The Q3 books. Refunds, the ledger export and anything finance is waiting on. Numbers first.",
  memory:
    "Q3 close finished October 9.\n" +
    "Refunds: Brightline paid October 8; Northwind and Halcyon still open with finance.\n" +
    "Ledger export (infra 77) stays blocked on the row limit; finance used the manual export for the close.\n" +
    "Decided: next quarter, start the close on the first business day, not the fifth.",
  notes: ["Halcyon refund: wait for their PO number before approving anything."],
  scope: { github_issues: ["ghost-mock/app"] },
  createdDaysAgo: 40,
  lastActiveMin: 60 * 24 * 9,
  settledDaysAgo: 9,
  turns: [
    ["assistant", "Q3 close."],
    ["user", "Where are we?"],
    ["assistant", "The close finished on the ninth. Two refunds are still open with finance: Northwind and Halcyon."],
    ["user", "Good. That's the quarter, then."],
    ["assistant", "Noted. Settle this line when you're ready and start Q4 fresh."],
  ],
  compactions: 2,
  history: [
    "Q3 close is running, target October 9. Three refunds open with finance (Northwind, Halcyon, Brightline).",
    "Q3 close finished October 9.\nRefunds: Brightline paid October 8; Northwind and Halcyon still open with finance.",
  ],
};

/** Busy only: a second settled line, so the group shows more than one. */
const LAUNCH: Seed = {
  slug: "launch",
  name: "Launch",
  aliases: ["the launch"],
  voice: "am_adam",
  speed: 1.1,
  persona: "The September launch. Blockers only; skip anything that can wait a week.",
  memory: "Shipped September 24. Two follow-ups filed against the app repo: 170 (onboarding copy) and 171 (pricing page cache).",
  notes: [],
  scope: { github_issues: ["ghost-mock/app", "ghost-mock/docs"] },
  createdDaysAgo: 48,
  lastActiveMin: 60 * 24 * 24,
  settledDaysAgo: 23,
  turns: [
    ["assistant", "Launch line."],
    ["user", "Anything blocking?"],
    ["assistant", "Nothing. Both follow-ups are filed and labeled."],
  ],
  compactions: 1,
};

const BUSY_ALIASES: Record<string, string[]> = {
  Hiring: ["recruiting"],
  "Infra on-call": ["on-call", "the pager"],
  "Design review": ["design"],
  "Release train": ["the release", "the train"],
  Security: ["sec"],
  "Customer escalations": ["escalations"],
  Personal: ["errands"],
};

const BUSY_EXTRA: [string, string, string][] = [
  ["Hiring", "Short. Candidate pipeline for the platform role; never repeat names out loud.", "af_nicole"],
  ["Infra on-call", "Pager context. What's on fire, what changed, who's on call.", "am_onyx"],
  ["Design review", "Read me the open design-review issues and who asked.", "af_sky"],
  ["Release train", "What's in the next release, what's still open against the milestone.", "bm_daniel"],
  ["Security", "CVEs, dependabot, anything labeled security. Be blunt.", "am_fenrir"],
  ["Customer escalations", "Escalations from support, newest first, with the customer name.", "af_bella"],
  ["Roadmap", "Long-horizon items only. Don't bring up bugs here.", "bf_alice"],
  ["Personal", "Not work. Reminders and errands.", "af_river"],
];

/** Earlier exchanges, already folded into memory. Cycled to give older history its length. */
const OLDER: [TurnView["role"], string][][] = [
  [["user", "Read me what changed since Friday."], ["assistant", "Since Friday: five issues closed, two new ones need your sign-off."], ["user", "Which two?"], ["assistant", "App 148, the Safari cookie bug, and infra 77, the ledger export timing out."]],
  [["user", "Anything waiting on me?"], ["assistant", "One: Dana asked you to look at app 140 before she merges."], ["user", "Tell her I'll look this afternoon."], ["assistant", "Commented on app 140: you'll review this afternoon."]],
  [["user", "What did we decide about the duplicate reports?"], ["assistant", "Close the newer one with a link to the older, no other comment."], ["user", "Do that for 133."], ["assistant", "Closed app 133 as a duplicate of 129."]],
  [["user", "Who owns the export bug?"], ["assistant", "Priya. Infra 77 is assigned to her and labeled blocked."], ["user", "Okay, leave it."], ["assistant", "Left it alone."]],
];

const LEGAL_OLDER: [TurnView["role"], string][][] = [
  [["user", "Where are we on the Northwind DPA?"], ["assistant", "Marco sent redlines Tuesday. Two open points: retention and the subprocessor list."], ["user", "Put both on infra 91."], ["assistant", "Commented on infra 91 with the two open points."]],
  [["user", "Anything new labeled needs-legal-review?"], ["assistant", "Two: app 162, the CSV export, and app 165, the new analytics script."], ["user", "Read me 165."], ["assistant", "App 165 adds a third-party analytics script to the billing page. Priya opened it."]],
  [["user", "Does the build image still ship the AGPL tools?"], ["assistant", "No. Both are build-time only since the change on infra 88."], ["user", "Good. Note that on 91."], ["assistant", "Commented on infra 91: the AGPL tools are build-time only."]],
  [["user", "Who signs off on data exports?"], ["assistant", "You do, then Marco. That's what we decided on app 150."], ["user", "Remind me when 162 is ready."], ["assistant", "I'll bring up app 162 when it's ready for review."]],
];

/** Turn ids are the store's global AUTOINCREMENT ids, shared by every line, so they're large and have gaps. */
let nextId = 4120;

function build(seed: Seed, live: boolean, older = seed.compactions * 4): LineRecord {
  const rand = seeded(seed.slug.length * 97 + seed.createdDaysAgo);
  const id = () => (nextId += 1 + Math.floor(rand() * 4));
  // The live line's latest turns are the current session's: minutes old, not yesterday's.
  const lastMin = live ? 1 : (seed.lastActiveMin ?? 60);
  const startMin = lastMin + seed.turns.length * 0.4;
  // When each version was written, oldest first. Version i folded the older turns up to
  // `covered(i)`; those turns were said in a session shortly before it.
  const at = Array.from({ length: seed.compactions }, (_, i) => 60 * 24 * (seed.compactions - i) * 1.5 + rand() * 120);
  // The newest version of a line compacted more than once is the operator's edit, which folds no turns.
  const folds = seed.compactions > 1 ? seed.compactions - 1 : 1;
  const covered = (i: number) => Math.min(older, Math.max(1, Math.round(((i + 1) / folds) * older)));
  const old: TurnView[] = [];
  for (let i = 0; i < older; i++) {
    const pool = seed.slug === "legal" ? LEGAL_OLDER : OLDER;
    const [role, text] = pool[Math.floor(i / 4) % pool.length][i % 4];
    let v = 0;
    while (v < at.length - 1 && i >= covered(v)) v++;
    const end = covered(v);
    old.push({ id: 0, role, text, at: ago(at[v] + 20 + (end - 1 - i) * 0.5), session_id: `s${v}`, compacted: true });
  }
  const recent: TurnView[] = seed.turns.map(([role, text], i) => ({
    id: 0,
    role,
    text,
    at: ago(startMin - i * 0.4),
    session_id: live ? SESSION_ID : "b7e2f1",
    compacted: false,
  }));
  const all = [...old, ...recent].map((t) => ({ ...t, id: id() }));
  // Every applied summary is a version (ADR 0014): "auto" from the in-session summarizer,
  // "manual" from Compact now, "edit" when the operator changed memory by hand. Each fold
  // covers more of the older turns; an edit folds none.
  const lines = seed.memory.split("\n");
  const compactions: CompactionView[] = Array.from({ length: seed.compactions }, (_, i) => {
    const newest = i === seed.compactions - 1;
    const source = newest && seed.compactions > 1 ? "edit" : i % 3 === 1 ? "manual" : "auto";
    return {
      id: `cmp-${seed.slug}-${i + 1}`,
      at: ago(at[i]),
      source,
      summary: newest ? seed.memory : (seed.history?.[i] ?? lines.slice(0, Math.max(1, lines.length - (seed.compactions - 1 - i))).join("\n")),
      through_turn_id: source === "edit" || !old.length ? null : all[Math.min(covered(i), old.length) - 1].id,
    };
  }).reverse();
  return {
    detail: {
      slug: seed.slug,
      name: seed.name,
      aliases: seed.aliases,
      voice: seed.voice,
      speed: seed.speed,
      persona: seed.persona,
      memory: seed.memory,
      notes: seed.notes,
      created_at: ago(60 * 24 * seed.createdDaysAgo),
      last_active_at: seed.lastActiveMin == null && !live ? null : ago(lastMin),
      active: false,
      is_ghost: seed.slug === "ghost",
      turn_count: all.length,
      compactions: seed.compactions,
      dir: `${HOME}/${seed.slug}`,
      scope: structuredClone(seed.scope ?? {}),
      settled_at: seed.settledDaysAgo != null ? ago(60 * 24 * seed.settledDaysAgo) : null,
    },
    turns: all,
    compactions,
  };
}

export interface ConversationsFixture {
  lines: LineRecord[];
  voices: VoiceInfo[];
}

export default function conversations(s: Scenario): ConversationsFixture {
  let seeds: Seed[];
  if (s.data === "empty") {
    seeds = [{ ...GHOST, persona: "", memory: "", notes: [], turns: [], compactions: 0, lastActiveMin: null, createdDaysAgo: 0 }];
  } else if (s.data === "busy") {
    seeds = [
      GHOST,
      ACCOUNTING,
      { ...LEGAL, createdDaysAgo: 34, compactions: 5, history: LEGAL_HISTORY },
      STANDUP,
      INFRA,
      APP,
      Q3_CLOSE,
      LAUNCH,
      ...BUSY_EXTRA.map(([name, persona, voice], i) => ({
        slug: name.toLowerCase().replace(/[^a-z0-9]+/g, "-"),
        name,
        aliases: BUSY_ALIASES[name] ?? [],
        voice,
        speed: 1,
        persona,
        memory: "",
        notes: [],
        createdDaysAgo: 30 - i * 3,
        lastActiveMin: 60 * (5 + i * 9),
        turns: [["assistant", `${name} line.`]] as [TurnView["role"], string][],
        compactions: i % 3,
      })),
    ];
  } else {
    seeds = [GHOST, ACCOUNTING, LEGAL, STANDUP, INFRA, APP, Q3_CLOSE];
  }
  nextId = 4120;
  // Busy: months on the Legal line, more than the 200 turns the screen asks for.
  const lines = seeds.map((seed) => build(seed, !!s.session && seed.slug === s.session, s.data === "busy" && seed.slug === "legal" ? 236 : undefined));
  for (const l of lines) l.detail.active = !!s.session && l.detail.slug === s.session;
  return { lines, voices: VOICES };
}

/** The 54 Kokoro v1.0 voices, labeled the way ghostd's voice_label() does. */
const VOICE_IDS = [
  "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore", "af_nicole", "af_nova", "af_river", "af_sarah", "af_sky",
  "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam", "am_michael", "am_onyx", "am_puck", "am_santa",
  "bf_alice", "bf_emma", "bf_isabella", "bf_lily", "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
  "jf_alpha", "jf_gongitsune", "jf_nezumi", "jf_tebukuro", "jm_kumo",
  "zf_xiaobei", "zf_xiaoni", "zf_xiaoxiao", "zf_xiaoyi", "zm_yunjian", "zm_yunxi", "zm_yunxia", "zm_yunyang",
  "ef_dora", "em_alex", "em_santa", "ff_siwis", "hf_alpha", "hf_beta", "hm_omega", "hm_psi",
  "if_sara", "im_nicola", "pf_dora", "pm_alex", "pm_santa",
];
const LANG: Record<string, string> = { a: "American", b: "British", e: "Spanish", f: "French", h: "Hindi", i: "Italian", j: "Japanese", p: "Portuguese", z: "Chinese" };

export const VOICES: VoiceInfo[] = VOICE_IDS.map((id) => {
  const [head, name] = id.split("_");
  const label = `${LANG[head[0]]} ${head[1] === "f" ? "female" : "male"}: ${name[0].toUpperCase()}${name.slice(1)}`;
  return { id, label };
});
