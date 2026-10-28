/**
 * Owner: conversations. One agent (#/agents/:slug): its persona and voice, its job, its
 * memory and pinned notes, its transcript and every version of its memory. Edits collect in
 * a draft and go in with Save; each field Ghost writes is an audited gate write, the same as
 * if it had been said. Pinning a note, compacting, switching, retiring, reset and delete
 * happen at once. Call opens a session with this agent and Hang up ends it; the composer in
 * the dock types to the agent, opening the session when there is none. Open folder reveals
 * the agent's files (ADR 0035).
 */
import { type CompactionView, type ConversationDetail, type ConversationView, type TurnView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Archive, ArchiveRestore, ArrowLeftRight, Brain, ChevronLeft, Clock, CloudOff, FolderOpen, Layers, MoreHorizontal, Phone, PhoneOff, Play, RotateCcw, SearchX, SlidersHorizontal, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Link, hrefFor, navigate } from "../../app/router";
import { useDesign } from "../../design/context";
import {
  Button,
  Card,
  EmptyState,
  Field,
  IconButton,
  Input,
  SaveBar,
  LineGlyph,
  LiveStatus,
  Menu,
  type MenuItem,
  Notice,
  Page,
  Section,
  Skeleton,
  SkeletonText,
  Slider,
  Textarea,
  agentName,
  errorText,
  quoted,
  relativeTime,
  useConfirm,
  useLive,
  useToast,
} from "../../ui";
import { type Draft, type DraftKey, dirtyKeys, drafts, fromDetail, listWords, rebase, updateBody } from "./draft";
import { useAutoRows } from "./autosize";
import { aliasExample, MEMORY_EXAMPLE, PERSONA_EXAMPLE } from "./examples";
import { LineStateText, lineState } from "./lineState";
import { History } from "./History";
import { Notes } from "./Notes";
import { personaCache } from "./personaCache";
import { Composer } from "../session/Composer";
import { rehireAgent, retireAgent, retiredAt } from "./threads";
import { Transcript } from "./Transcript";
// The composer's styles live with the session screen, which may not have been opened yet.
import "../session/session.css";
import { claimParams, clearParams, designFail, designRetry, focusOnOpen, newAgentSeed, readParam, writeParam } from "./url";
import { PlayingBars, VoicePicker, voiceTitle } from "./VoicePicker";
import { type Voice, parseVoice, voiceKind } from "./voices";
import { revealFolder } from "../watching/model";

type TabId = "persona" | "memory" | "transcript" | "history";
const TABS: TabId[] = ["persona", "memory", "transcript", "history"];
const TURN_LIMIT = 200;
const PERSONA_KEYS: DraftKey[] = ["name", "aliases", "job", "voice", "speed", "persona"];
const MEMORY_KEYS: DraftKey[] = ["memory", "notes"];

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

const times = (n: number) => (n === 1 ? "once" : n === 2 ? "twice" : `${n} times`);

export function AgentDetail({ slug }: { slug: string }) {
  const { client, status, conversationsVersion, turns: sessionTurns, pending } = useGhost();
  const { online, connection, phase, loadingModels } = useLive();
  const design = useDesign();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();

  const [detail, setDetail] = useState<ConversationDetail | null>(null);
  const [base, setBase] = useState<Draft | null>(null);
  const [draft, setDraftState] = useState<Draft | null>(null);
  const [turns, setTurns] = useState<TurnView[]>([]);
  const [versions, setVersions] = useState<CompactionView[]>([]);
  const [voices, setVoices] = useState<Voice[]>([]);
  /** Every agent, for the live one's name in the composer. */
  const [agents, setAgents] = useState<ConversationView[]>([]);
  const [missing, setMissing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [notice, setNotice] = useState<{ title: string; text: string } | null>(null);
  const [tab, setTabState] = useState<TabId>(() => {
    const t = readParam("tab", slug) as TabId | null;
    return t && TABS.includes(t) ? t : "transcript";
  });
  const [picker, setPicker] = useState(false);
  const [saving, setSaving] = useState(false);
  const [hearing, setHearing] = useState<string | null>(null);
  const [pinning, setPinning] = useState(false);
  const [acting, setActing] = useState<"switch" | "compact" | "retire" | "rehire" | "call" | "hangup" | "send" | null>(null);
  const [text, setText] = useState("");
  const composerRef = useRef<HTMLTextAreaElement>(null);

  const inSession = !!status?.session_id;
  const liveSlug = inSession ? (status?.conversation ?? "ghost") : null;
  const live = liveSlug === slug;
  const isGhost = slug === "ghost";
  const name = agentName(slug, detail?.name);
  // Retired (ADR 0024): off the roster and voice routing, everything readable, nothing editable.
  const retired = retiredAt(detail);
  const readOnly = retired !== null;

  const setTab = (t: TabId) => {
    setTabState(t);
    writeParam("tab", t === "transcript" ? null : t, slug);
  };
  // The view and dialog params belong to this agent: another agent's leftovers are dropped on
  // the way in, and nothing outlives the screen on the way out.
  useEffect(() => {
    claimParams(slug);
  }, [slug]);
  useEffect(
    () => () => {
      if (!window.location.hash.startsWith("#/agents/")) clearParams();
    },
    [],
  );

  // ---------------------------------------------------------------- data
  const baseRef = useRef<Draft | null>(null);
  const draftRef = useRef<Draft | null>(null);
  const apply = useCallback(
    (d: ConversationDetail, keepDraft = true) => {
      const next = fromDetail(d);
      personaCache.set(slug, d.persona);
      const stored = drafts.get(slug);
      const prevDraft = draftRef.current ?? stored?.draft ?? null;
      const prevBase = baseRef.current ?? stored?.base ?? null;
      const merged = keepDraft && prevDraft && prevBase ? rebase(prevDraft, prevBase, next) : next;
      baseRef.current = next;
      draftRef.current = merged;
      setDetail(d);
      setBase(next);
      setDraftState(merged);
      if (dirtyKeys(merged, next, d.is_ghost).length) drafts.set(slug, { draft: merged, base: next });
      else drafts.delete(slug);
    },
    [slug],
  );

  const setDraft = (patch: Partial<Draft>) => {
    if (!draftRef.current || !baseRef.current || !detail) return;
    const next = { ...draftRef.current, ...patch };
    draftRef.current = next;
    setDraftState(next);
    if (dirtyKeys(next, baseRef.current, detail.is_ghost).length) drafts.set(slug, { draft: next, base: baseRef.current });
    else drafts.delete(slug);
  };

  // A new slug starts clean (its own stored draft, if any, is picked up by apply()).
  useEffect(() => {
    baseRef.current = null;
    draftRef.current = null;
    setDetail(null);
    setBase(null);
    setDraftState(null);
    setTurns([]);
    setVersions([]);
    setMissing(false);
    setLoadError(null);
    setNotice(null);
  }, [slug]);

  const load = useCallback(async () => {
    try {
      if (designFail(design, "agent")) throw new Error("Ghost couldn’t read the agent (database is locked).");
      const [d, t, c] = await Promise.all([
        client.conversation(slug),
        client.conversationTurns(slug, TURN_LIMIT),
        client.conversationCompactions(slug).catch(() => [] as CompactionView[]),
      ]);
      apply(d);
      setTurns(t);
      setVersions(c);
      setMissing(false);
      setLoadError(null);
    } catch (e) {
      if ((e as { status?: number } | null)?.status === 404) setMissing(true);
      else setLoadError(errorText(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, slug, apply]);

  // While this agent is live its transcript grows with the session.
  const liveTurns = live ? sessionTurns.length : 0;
  useEffect(() => {
    void load();
  }, [load, conversationsVersion, liveTurns]);

  useEffect(() => {
    client
      .voices()
      .then((v) => setVoices(v.map(parseVoice)))
      .catch(() => setVoices([]));
  }, [client, status?.models_ready]);

  useEffect(() => {
    let on = true;
    client
      .conversations()
      .then((l) => on && setAgents(l))
      .catch(() => {});
    return () => {
      on = false;
    };
  }, [client, conversationsVersion]);

  // ---------------------------------------------------------------- derived
  const dirty = useMemo(() => (draft && base && detail ? dirtyKeys(draft, base, detail.is_ghost) : []), [draft, base, detail]);
  // The voice heard: the agent's own when it has one, else Ghost's (the detail reports the one in force).
  const heardId = draft?.voice || detail?.voice || "af_heart";
  const voice = draft ? (voices.find((v) => v.id === heardId) ?? parseVoice(heardId)) : null;
  const pickable = useMemo(() => (draft && !voices.some((v) => v.id === heardId) ? [...voices, parseVoice(heardId)] : voices), [voices, draft, heardId]);
  const hearBlocked = !online ? "Ghost isn’t answering right now." : !status?.models_ready ? "Hear it works once the voice models finish loading." : null;
  const lastVersion = [...versions].sort((a, b) => b.at.localeCompare(a.at))[0];

  // ---------------------------------------------------------------- actions
  const fail = (title: string, e: unknown) => setNotice({ title, text: errorText(e) });

  const save = async () => {
    if (!draft || !dirty.length || saving || !online || readOnly) return;
    setSaving(true);
    setNotice(null);
    const words = listWords(dirty);
    try {
      const d = await client.updateConversation(slug, updateBody(draft, dirty));
      drafts.delete(slug);
      apply(d, false);
      toast({ title: "Saved", description: `${name}’s ${words} ${dirty.length === 1 && !["notes", "aliases"].includes(dirty[0]) ? "is" : "are"} updated. Each change is in the audit log.`, tone: "ok" });
    } catch (e) {
      fail("Couldn’t save", e);
      void load();
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    if (!base) return;
    drafts.delete(slug);
    draftRef.current = base;
    setDraftState(base);
  };

  // One sample at a time. A voice asked for while another plays waits its turn; only the
  // newest waits, so walking the picker with "Play as I pick" on never builds a queue.
  const hearingRef = useRef<string | null>(null);
  const nextRef = useRef<string | null>(null);
  const hear = async (id: string) => {
    if (hearBlocked) return;
    if (hearingRef.current) {
      if (hearingRef.current !== id) nextRef.current = id;
      return;
    }
    hearingRef.current = id;
    setHearing(id);
    try {
      const r = await client.sampleVoice(id, draftRef.current?.speed ?? 1);
      if (!r.ok) setNotice({ title: "Couldn’t play a sample", text: "Ghost may be speaking. Try again in a moment." });
    } catch (e) {
      nextRef.current = null;
      fail("Couldn’t play a sample", e);
    } finally {
      hearingRef.current = null;
      setHearing(null);
      const next = nextRef.current;
      nextRef.current = null;
      if (next) void hear(next);
    }
  };

  const pin = async (text: string) => {
    setPinning(true);
    try {
      apply(await client.addNote(slug, text));
      toast({ title: "Pinned", description: `${name} will keep it word for word.` });
      return true;
    } catch (e) {
      fail("Couldn’t pin that", e);
      return false;
    } finally {
      setPinning(false);
    }
  };

  // Call: a session with this agent, the way a wake is one with Ghost. Hang up ends whichever is live.
  const modelsReady = !!status?.models_ready && !loadingModels;
  const call = async () => {
    setActing("call");
    try {
      const r = await client.wake(undefined, slug);
      if (r.ok === false) setNotice({ title: `Couldn’t call ${name}`, text: r.detail ?? "Ghost didn’t take the call. Try again." });
    } catch (e) {
      fail(`Couldn’t call ${name}`, e);
    } finally {
      setActing(null);
    }
  };
  const hangUp = async () => {
    setActing("hangup");
    try {
      await client.sleep();
    } catch (e) {
      fail("Couldn’t hang up", e);
    } finally {
      setActing(null);
    }
  };
  // Typed to this agent: into the live session when it is here, else a new session with this agent.
  const send = async () => {
    const t = text.trim();
    if (!t || acting) return;
    setText("");
    setActing("send");
    try {
      const r = live ? await client.say(t) : await client.say(t, slug);
      if (r.ok === false) {
        setText((now) => now || t);
        setNotice({ title: `Couldn’t send to ${name}`, text: r.detail ?? "Ghost didn’t take it. Try again." });
      }
    } catch (e) {
      setText((now) => now || t);
      fail(`Couldn’t send to ${name}`, e);
    } finally {
      setActing(null);
      composerRef.current?.focus();
    }
  };

  const switchTo = async (target: string) => {
    const to = target === slug ? name : agentName(target);
    setActing("switch");
    try {
      const r = await client.switchConversation(target);
      if (r.ok === false) setNotice({ title: `Couldn’t switch to ${to}`, text: r.detail ?? "Ghost didn’t take the switch. Try again, or say it." });
      else toast({ title: `Transferred to ${to}`, description: target === "ghost" ? "You’re back with Ghost." : `The session is with ${to} now.` });
    } catch (e) {
      fail(`Couldn’t switch to ${to}`, e);
    } finally {
      setActing(null);
    }
  };

  const compact = async () => {
    setActing("compact");
    try {
      await client.compactConversation(slug);
      toast({ title: `Compacting ${name}`, description: "The new memory shows up here when Ghost finishes." });
    } catch (e) {
      fail("Couldn’t compact", e);
    } finally {
      setActing(null);
    }
  };

  const reset = async () => {
    const ok = await confirm({
      title: `Reset ${name}?`,
      description: `Ghost forgets ${name}’s memory, pinned notes and transcript. The name, job, voice and persona stay.`,
      confirmLabel: `Reset ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    try {
      await client.resetConversation(slug);
      drafts.delete(slug);
      baseRef.current = null;
      draftRef.current = null;
      await load();
      toast({ title: `Reset ${name}`, description: "Memory, notes and transcript are cleared." });
    } catch (e) {
      fail(`Couldn’t reset ${name}`, e);
    }
  };

  // Retire: announce-level, nothing destroyed (ADR 0024). Unsaved edits can't be kept on a
  // read-only agent, so they are dropped with the retire; the dialog says so when there are any.
  const retire = async () => {
    const ok = await confirm({
      title: `Retire ${name}?`,
      description: `${name} leaves voice routing and the roster. Its transcript, memory and notes stay here to read, and Rehire brings it back.${dirty.length ? " Your unsaved changes here are dropped." : ""}`,
      confirmLabel: `Retire ${name}`,
    });
    if (!ok) return;
    setActing("retire");
    try {
      await retireAgent(client, slug);
      drafts.delete(slug);
      baseRef.current = null;
      draftRef.current = null;
      await load();
      toast({ title: `Retired ${name}`, description: "It stays readable here and is out of voice routing. Rehire brings it back." });
    } catch (e) {
      fail(`Couldn’t retire ${name}`, e);
    } finally {
      setActing(null);
    }
  };

  const rehire = async () => {
    setActing("rehire");
    try {
      await rehireAgent(client, slug);
      await load();
      toast({ title: `${name} is back`, description: `Say ${quoted(`transfer me to ${name.toLowerCase()}`)} to reach it again.` });
    } catch (e) {
      fail(`Couldn’t bring ${name} back`, e);
    } finally {
      setActing(null);
    }
  };

  const openFolder = async () => {
    if (!detail) return;
    const r = await revealFolder(detail.dir);
    if (!r.ok) toast({ title: "The agent’s folder", description: `${r.path} (open it in your file manager; this build can’t open folders itself).` });
  };

  const remove = async () => {
    const ok = await confirm({
      title: `Delete ${name}?`,
      description: `The agent goes, with its folder: persona, memory, pinned notes and transcript. Retire keeps them.`,
      confirmLabel: `Delete ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    try {
      await client.deleteConversation(slug);
      drafts.delete(slug);
      toast({ title: `Deleted ${name}` });
      navigate("/agents");
    } catch (e) {
      fail(`Couldn’t delete ${name}`, e);
    }
  };

  // Ctrl S saves, from anywhere on the page.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  // Design mode: open a dialog or make a draft from the URL, for screenshots of each state.
  // The destructive dialogs open only with the mock, and never over an agent Ghost is with.
  const opened = useRef(false);
  useEffect(() => {
    if (!detail || opened.current) return;
    opened.current = true;
    const o = readParam("open", slug);
    if (o === "voice") setPicker(true);
    if (import.meta.env.DEV && design && !live) {
      if (o === "reset") void reset();
      if (o === "delete" && !isGhost) void remove();
      if (o === "retire" && !isGhost && !readOnly) void retire();
    }
    if (design && readParam("dirty") && draftRef.current) {
      setDraft({ persona: `${draftRef.current.persona}\nRead amounts in dollars, never cents.`.trim(), voice: "bm_lewis" });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail]);

  // An agent just hired opens with focus in its persona (set by the index before navigating).
  const personaRef = useRef<HTMLTextAreaElement>(null);
  const memoryRef = useRef<HTMLTextAreaElement>(null);
  const personaRows = useAutoRows(personaRef, draft?.persona ?? "", 2, 16);
  const memoryRows = useAutoRows(memoryRef, draft?.memory ?? "", 3, 16);
  useEffect(() => {
    if (!draft || focusOnOpen.slug !== slug) return;
    focusOnOpen.slug = null;
    setTab("persona");
    requestAnimationFrame(() => personaRef.current?.focus({ preventScroll: true }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft != null, slug]);

  const retry = async () => {
    setRetrying(true);
    designRetry("agent");
    await load();
    setRetrying(false);
  };

  // ---------------------------------------------------------------- render
  if (missing) {
    return (
      <Page width="default" title="Agent not found" bare>
        <EmptyState
          size="page"
          icon={<SearchX />}
          title={`There’s no agent called ${quoted(slug)}`}
          description="It may have been deleted, or the link has a typo."
          actions={<Button href="#/agents">See all agents</Button>}
        />
      </Page>
    );
  }

  const meta: string[] = [];
  if (detail) {
    if (isGhost) meta.push("Every wake lands here");
    else if (detail.job) meta.push(detail.job);
    if (retired) meta.push(`retired ${relativeTime(retired)}`);
    if (!live) meta.push(detail.last_active_at ? `last used ${relativeTime(detail.last_active_at)}` : "not used yet");
    if (detail.turn_count) {
      meta.push(plural(detail.turn_count, "turn"));
      meta.push(detail.compactions ? `compacted ${times(detail.compactions)}` : "never compacted");
    }
  }
  // The page status is the agent's own state while it is live, in the sidebar's words
  // ("Waiting on your okay" in amber while a change waits here, else "Live · listening"), and
  // Ghost's state otherwise, like every other screen. Either way it stays in the compact bar
  // when the header scrolls away. The description keeps the agent's facts.
  const state = lineState({ live, pending: !!pending, connection, phase, lastActive: detail?.last_active_at ?? null });
  const waiting = state.tone === "waiting";
  const pageStatus = live ? <LineStateText state={state} connection={connection} phase={phase} /> : <LiveStatus />;
  const description = detail ? (
    <span className="conv-meta">{meta.map((m, i) => (i === 0 ? m[0].toUpperCase() + m.slice(1) : m)).join(" · ")}</span>
  ) : loadError ? undefined : (
    <span className="conv-meta-skel">
      <Skeleton width={240} height={14} />
    </span>
  );

  // Reset and delete need the session elsewhere. On the live agent the menu says so and offers
  // the way out (back to Ghost). Every agent keeps them behind the same More menu; Ghost
  // has no Retire or Delete, and while it is live its Reset is disabled with the reason.
  const mark = (keys: DraftKey[]) => (dirty.some((k) => keys.includes(k)) ? " (unsaved)" : "");
  const settingsItems: MenuItem[] = [
    { heading: "Settings" },
    { label: `Voice and persona${mark(PERSONA_KEYS)}`, icon: <SlidersHorizontal />, onSelect: () => setTab("persona") },
    { label: `Memory${mark(MEMORY_KEYS)}`, icon: <Brain />, onSelect: () => setTab("memory") },
    { label: "History", icon: <Clock />, onSelect: () => setTab("history") },
    { label: "Open folder", icon: <FolderOpen />, onSelect: () => void openFolder() },
    "separator",
  ];
  const busyHere = { disabled: true, reason: "Ghost is with this agent" };
  const menu = detail && (
    <Menu
      align="end"
      trigger={<IconButton label={`More for ${name}`} icon={<MoreHorizontal />} variant="secondary" disabled={!online} />}
      items={[
        ...settingsItems,
        ...((isGhost
          ? [live ? { label: `Reset ${name}…`, icon: <RotateCcw />, onSelect: () => void reset(), ...busyHere } : { label: `Reset ${name}…`, icon: <RotateCcw />, onSelect: () => void reset() }]
          : readOnly
            ? [
                { label: `Rehire ${name}`, icon: <ArchiveRestore />, onSelect: () => void rehire() },
                "separator",
                { label: `Delete ${name}…`, icon: <Trash2 />, danger: true, onSelect: () => void remove() },
              ]
          : live
            ? [
                { label: "Switch to Ghost", icon: <ArrowLeftRight />, onSelect: () => void switchTo("ghost") },
                "separator",
                { label: `Retire ${name}…`, icon: <Archive />, onSelect: () => void retire(), ...busyHere },
                { label: `Reset ${name}…`, icon: <RotateCcw />, onSelect: () => void reset(), ...busyHere },
                { label: `Delete ${name}…`, icon: <Trash2 />, onSelect: () => void remove(), ...busyHere },
              ]
            : [
                { label: `Retire ${name}…`, icon: <Archive />, onSelect: () => void retire() },
                { label: `Reset ${name}…`, icon: <RotateCcw />, onSelect: () => void reset() },
                "separator",
                { label: `Delete ${name}…`, icon: <Trash2 />, danger: true, onSelect: () => void remove() },
              ]) as MenuItem[]),
      ]}
    />
  );

  const actions = detail && (
    <>
      {/* While a change waits, reviewing it is the one thing to do here; the save bar's Save keeps the primary when there are edits too. */}
      {waiting && (
        <Button variant={dirty.length ? "secondary" : "primary"} href={hrefFor("/")}>
          Review the change
        </Button>
      )}
      {/* Nothing live: Call opens a session here. Live here: Hang up. Live elsewhere: transfer it. */}
      {!inSession && !readOnly && (
        <Button
          variant={waiting || dirty.length ? "secondary" : "primary"}
          leadingIcon={<Phone />}
          loading={acting === "call"}
          disabled={!online || !modelsReady || (acting !== null && acting !== "call")}
          title={!online ? "Ghost is reconnecting" : !modelsReady ? "Ghost is still loading its voice models" : undefined}
          onClick={() => void call()}
        >
          Call {name}
        </Button>
      )}
      {inSession && !live && !readOnly && (
        <Button variant="secondary" leadingIcon={<ArrowLeftRight />} loading={acting === "switch"} disabled={!online} onClick={() => void switchTo(slug)}>
          Switch to {name}
        </Button>
      )}
      {/* On a retired agent, bringing it back is the one thing to do here. */}
      {readOnly && (
        <Button variant="primary" leadingIcon={<ArchiveRestore />} loading={acting === "rehire"} disabled={!online} onClick={() => void rehire()}>
          Rehire
        </Button>
      )}
      {live && (
        <Button variant="secondary" leadingIcon={<Layers />} loading={acting === "compact"} disabled={!online} onClick={() => void compact()}>
          Compact now
        </Button>
      )}
      {live && (
        <Button variant="secondary" leadingIcon={<PhoneOff />} loading={acting === "hangup"} disabled={!online || (acting !== null && acting !== "hangup")} onClick={() => void hangUp()}>
          Hang up
        </Button>
      )}
      {menu}
    </>
  );

  const unsaved = (keys: DraftKey[]) =>
    dirty.some((k) => keys.includes(k)) ? (
      <>
        <span className="conv-tab-dot" aria-hidden />
        <span className="sr-only">, unsaved</span>
      </>
    ) : null;

  // Typing to the agent: on when the session is here or there is none; while Ghost is with
  // another agent the field says so, and Switch in the header is the way over.
  const elsewhere = inSession && !live;
  const canType = online && modelsReady && !elsewhere && (!inSession || live);
  const composerDock = detail && !readOnly && tab === "transcript" && (
    <div className="conv-dock">
      <Composer
        ref={composerRef}
        value={text}
        onChange={setText}
        onSubmit={() => void send()}
        disabled={!canType || (acting !== null && acting !== "send")}
        sending={acting === "send"}
        placeholder={!online ? "Reconnecting…" : elsewhere ? `Ghost is with ${agentName(liveSlug!, agents.find((l) => l.slug === liveSlug)?.name)}` : `Type to ${name}…`}
        label={`Message ${name}`}
        presence={live ? phase : undefined}
      />
    </div>
  );
  const dock =
    dirty.length > 0 ? (
      <SaveBar
        variant={!online ? "offline" : "dirty"}
        title={`${dirty.length} unsaved change${dirty.length === 1 ? "" : "s"}`}
        detail={!online ? "they wait here until Ghost is back" : listWords(dirty)}
        busy={saving}
        onDiscard={discard}
        onSave={() => void save()}
      />
    ) : composerDock || undefined;

  return (
    <Page
      width="default"
      className="conv-page"
      title={name}
      leading={<LineGlyph slug={slug} name={name} size="xl" />}
      crumb={
        <Link to="/agents">
          <ChevronLeft size={14} aria-hidden />
          Agents
        </Link>
      }
      status={pageStatus}
      description={description}
      actions={actions}
      dock={dock}
    >
      {notice && (
        <Notice tone="danger" title={notice.title} onDismiss={() => setNotice(null)} className="conv-notice">
          {notice.text}
        </Notice>
      )}

      {retired && (
        <p className="conv-settled t-small">
          <Archive size={14} aria-hidden />
          <span>
            Retired {relativeTime(retired)}. Read-only until you bring it back: {quoted(`transfer me to ${name.toLowerCase()}`)} does nothing, and nothing here can change.
          </span>
        </p>
      )}

      {loadError && !detail ? (
        <Card>
          <EmptyState
            icon={<CloudOff />}
            title={`Couldn’t load ${name}`}
            description={
              <>
                Ghost didn’t send this agent. Try again in a moment.
                <span className="conv-error-detail t-small t-3">{loadError}</span>
              </>
            }
            actions={
              <Button loading={retrying} onClick={() => void retry()}>
                Try again
              </Button>
            }
          />
        </Card>
      ) : (
      <div className="conv-view">
        {tab !== "transcript" && (
          <div className="conv-view__bar">
            <Button variant="ghost" size="sm" onClick={() => setTab("transcript")}>
              ← Conversation
            </Button>
            <span className="conv-view__title">
              {tab === "persona" ? <>Voice and persona{unsaved(PERSONA_KEYS)}</> : tab === "memory" ? <>Memory{unsaved(MEMORY_KEYS)}</> : "History"}
            </span>
          </div>
        )}
        {!detail || !draft ? (
          <DetailSkeleton tab={tab} />
        ) : tab === "persona" ? (
          <>
            <Section
              id="conv-persona"
              title="Persona"
              description={
                isGhost ? (
                  <>Ghost is where every wake starts, and {quoted("back to Ghost")} returns here. Its persona is in your words and goes into the prompt as written; Ghost never rewrites it.</>
                ) : (
                  "In your words. It goes into the prompt as written; Ghost never rewrites it."
                )
              }
            >
              <Textarea
                ref={personaRef}
                aria-labelledby="conv-persona-title"
                rows={personaRows}
                className="conv-prompt"
                data-user-content
                value={draft.persona}
                placeholder={PERSONA_EXAMPLE}
                disabled={readOnly}
                onChange={(e) => setDraft({ persona: e.target.value })}
              />
            </Section>

            <Section id="conv-voice" title="Voice" description={isGhost ? "Ghost’s voice, which every agent without its own speaks in." : draft.voice ? "Its own voice." : "Ghost’s voice, until you give it one of its own."}>
              <Card list>
                <div className="conv-voice">
                  <IconButton
                    variant="secondary"
                    className="conv-voice__play"
                    label={hearing === heardId ? `Playing ${voice ? voiceTitle(voice, voices) : heardId}` : `Hear ${voice ? voiceTitle(voice, voices) : heardId}`}
                    icon={hearing === heardId ? <PlayingBars /> : <Play />}
                    disabled={!!hearBlocked || (hearing != null && hearing !== heardId)}
                    onClick={() => void hear(heardId)}
                  />
                  <div className="conv-voice__text">
                    <p className="t-medium">{voice && voiceTitle(voice, voices)}</p>
                    <p className="t-small t-2" aria-live="polite">
                      {hearing === heardId && !picker ? (
                        <span className="conv-playing">Playing on your speakers…</span>
                      ) : (
                        <>
                          {voice && voiceKind(voice)}
                          <span className="t-3"> · </span>
                          <span className="t-mono t-3">{heardId}</span>
                          {!draft.voice && !isGhost && <span className="t-3"> · Ghost’s</span>}
                        </>
                      )}
                    </p>
                  </div>
                  {draft.voice && !isGhost && (
                    <Button variant="ghost" disabled={readOnly} onClick={() => setDraft({ voice: "" })}>
                      Use Ghost’s voice
                    </Button>
                  )}
                  <Button variant="secondary" disabled={readOnly} onClick={() => setPicker(true)}>
                    {draft.voice ? "Change voice" : "Give it a voice"}
                  </Button>
                </div>
                <Field layout="inline" label="Speed" help="1.00× is the voice’s natural pace." className="conv-cell">
                  <Slider min={0.5} max={2} step={0.05} value={draft.speed} disabled={readOnly} format={(v) => `${v.toFixed(2)}×`} onChange={(v) => setDraft({ speed: v })} />
                </Field>
              </Card>
              {hearBlocked && <p className="t-small t-3">{hearBlocked}</p>}
            </Section>

            <Section id="conv-name" title="How you reach it" description={<>What you say in a session: {quoted(`transfer me to ${(draft.name.trim() || name).toLowerCase()}`)}.</>}>
              <Card list>
                {!isGhost && (
                  <Field layout="inline" label="Name" className="conv-cell" error={draft.name.trim() ? undefined : "An agent needs a name."}>
                    <Input value={draft.name} placeholder={name} disabled={readOnly} onChange={(e) => setDraft({ name: e.target.value })} />
                  </Field>
                )}
                {!isGhost && (
                  <Field layout="inline" label="Also answers to" help="Other names, separated by commas." className="conv-cell">
                    <Input value={draft.aliases} placeholder={aliasExample(draft.name || name)} disabled={readOnly} onChange={(e) => setDraft({ aliases: e.target.value })} />
                  </Field>
                )}
                {!isGhost && (
                  <Field layout="inline" label="Job" help="What it was hired for, in a line." className="conv-cell">
                    <Input value={draft.job} placeholder="brand design" maxLength={200} disabled={readOnly} onChange={(e) => setDraft({ job: e.target.value })} />
                  </Field>
                )}
                {isGhost && (
                  <div className="conv-cell">
                    <p className="t-small t-2">Ghost keeps its name.</p>
                  </div>
                )}
              </Card>
            </Section>
          </>
        ) : tab === "memory" ? (
          <>
            <Section
              id="conv-memory"
              title="Memory"
              description={
                readOnly
                  ? `The summary this agent ended with. Start a new agent from it to carry it on.${lastVersion ? ` Updated ${relativeTime(lastVersion.at)}.` : ""}`
                  : `The running summary Ghost keeps for this agent. Compacting rewrites it; your edits are folded into the next version.${lastVersion ? ` Updated ${relativeTime(lastVersion.at)}.` : ""}`
              }
            >
              <Textarea
                ref={memoryRef}
                aria-labelledby="conv-memory-title"
                rows={memoryRows}
                className="conv-prompt"
                data-user-content
                value={draft.memory}
                placeholder={MEMORY_EXAMPLE}
                disabled={readOnly}
                onChange={(e) => setDraft({ memory: e.target.value })}
              />
              {readOnly && (
                <div className="conv-handoff">
                  <Button
                    variant="secondary"
                    onClick={() => {
                      newAgentSeed.slug = slug;
                      navigate("/agents");
                    }}
                  >
                    Start an agent from this memory
                  </Button>
                </div>
              )}
            </Section>
            <Section id="conv-notes" title="Pinned notes" description={<>Kept word for word and never summarized. Say {quoted("remember this")} in a session, or pin one here.</>}>
              <Notes notes={draft.notes} saved={base?.notes ?? []} onChange={(notes) => setDraft({ notes })} onPin={pin} pinning={pinning} disabled={!online || readOnly} />
            </Section>
            <p className="conv-files t-small t-3">
              Persona, memory and notes are files in <span className="t-mono">{detail.dir}</span>. Memory is this agent’s own; no other agent reads it.
            </p>
          </>
        ) : tab === "transcript" ? (
          <Transcript turns={turns} lineName={name} limit={TURN_LIMIT} total={detail.turn_count} />
        ) : (
          <History
            versions={versions}
            turns={turns}
            whole={turns.length < TURN_LIMIT}
            live={live}
            lineName={name}
            readOnly={readOnly}
            onUse={(c) => {
              setDraft({ memory: c.summary });
              setTab("memory");
              // No toast: it would sit over the save bar, which already says what changed.
              requestAnimationFrame(() => memoryRef.current?.focus({ preventScroll: true }));
            }}
          />
        )}
      </div>
      )}

      {draft && (
        <VoicePicker
          open={picker}
          onClose={() => {
            setPicker(false);
            writeParam("open", null, slug);
          }}
          lineName={name}
          voices={pickable}
          value={heardId}
          onPick={(id) => setDraft({ voice: id })}
          hearBlocked={hearBlocked}
          hearing={hearing}
          onHear={(id) => void hear(id)}
        />
      )}
      {dialog}
    </Page>
  );
}

/** Shaped like the tab that is loading: a persona field, memory and notes, bubbles, versions. */
function DetailSkeleton({ tab }: { tab: TabId }) {
  if (tab === "transcript") {
    return (
      <div className="conv-skel-turns" aria-busy="true" aria-label="Loading">
        <Skeleton width={280} height={12} className="conv-skel-marker" />
        {[180, 300, 240, 340, 220].map((w, i) => (
          <Skeleton key={i} width={w} height={i % 2 ? 36 : 56} radius="var(--radius-xl)" className={i % 2 ? "conv-skel-you" : undefined} />
        ))}
      </div>
    );
  }
  if (tab === "history") {
    return (
      <Card list aria-busy="true" aria-label="Loading">
        {[0, 1, 2].map((i) => (
          <div key={i} className="conv-version">
            <Skeleton width={i ? 180 : 220} height={14} />
            <SkeletonText lines={2} />
          </div>
        ))}
      </Card>
    );
  }
  return (
    <div className="stack gap-6" aria-busy="true" aria-label="Loading">
      <div className="stack gap-3">
        <Skeleton width={tab === "memory" ? 72 : 96} height={16} />
        <Skeleton height={tab === "memory" ? 120 : 96} radius="var(--radius-md)" />
      </div>
      <div className="stack gap-3">
        <Skeleton width={tab === "memory" ? 112 : 64} height={16} />
        <Card>
          <SkeletonText lines={2} />
        </Card>
      </div>
    </div>
  );
}
