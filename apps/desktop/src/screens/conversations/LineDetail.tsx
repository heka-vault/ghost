/**
 * Owner: conversations. One line (#/lines/:slug): its persona and voice, its
 * memory and pinned notes, its transcript and every version of its memory. Edits collect
 * in a draft and go in with Save; each field Ghost writes is an audited gate
 * write, the same as if it had been said. Pinning a note, compacting, switching, reset and
 * delete happen at once.
 */
import { type CompactionView, type ConversationDetail, type TurnView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Archive, ArchiveRestore, ArrowLeftRight, Brain, ChevronLeft, Clock, CloudOff, Layers, MoreHorizontal, Play, RotateCcw, SearchX, SlidersHorizontal, Trash2 } from "lucide-react";
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
  errorText,
  lineName,
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
import { ScopePicker } from "./ScopePicker";
import { type ScopedSource, scopedSources, settleLine, settledAt, unsettleLine } from "./threads";
import { Transcript } from "./Transcript";
import { claimParams, clearParams, designFail, designRetry, focusOnOpen, newLineSeed, readParam, writeParam } from "./url";
import { PlayingBars, VoicePicker, voiceTitle } from "./VoicePicker";
import { type Voice, parseVoice, voiceKind } from "./voices";

type TabId = "persona" | "memory" | "transcript" | "history";
const TABS: TabId[] = ["persona", "memory", "transcript", "history"];
const TURN_LIMIT = 200;
const PERSONA_KEYS: DraftKey[] = ["name", "aliases", "voice", "speed", "persona", "scope"];
const MEMORY_KEYS: DraftKey[] = ["memory", "notes"];

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const times = (n: number) => (n === 1 ? "once" : n === 2 ? "twice" : `${n} times`);

export function LineDetail({ slug }: { slug: string }) {
  const { client, status, conversationsVersion, turns: sessionTurns, pending } = useGhost();
  const { online, connection, phase } = useLive();
  const design = useDesign();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();

  const [detail, setDetail] = useState<ConversationDetail | null>(null);
  const [base, setBase] = useState<Draft | null>(null);
  const [draft, setDraftState] = useState<Draft | null>(null);
  const [turns, setTurns] = useState<TurnView[]>([]);
  const [versions, setVersions] = useState<CompactionView[]>([]);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [sources, setSources] = useState<ScopedSource[] | null>(null);
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
  const [acting, setActing] = useState<"switch" | "compact" | "settle" | "unsettle" | null>(null);

  const inSession = !!status?.session_id;
  const liveSlug = inSession ? (status?.conversation ?? "ghost") : null;
  const live = liveSlug === slug;
  const isGhost = slug === "ghost";
  const name = lineName(slug, detail?.name);
  // Settled (ADR 0019): out of voice routing and the active list, everything readable, nothing editable.
  const settled = settledAt(detail);
  const readOnly = settled !== null;

  const setTab = (t: TabId) => {
    setTabState(t);
    writeParam("tab", t === "transcript" ? null : t, slug);
  };
  // The view and dialog params belong to this line: another line's leftovers are dropped on
  // the way in, and nothing outlives the screen on the way out.
  useEffect(() => {
    claimParams(slug);
  }, [slug]);
  useEffect(
    () => () => {
      if (!window.location.hash.startsWith("#/lines/")) clearParams();
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
      if (designFail(design, "line")) throw new Error("Ghost couldn’t read the conversations database (database is locked).");
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

  // While this line is live its transcript grows with the session.
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

  // Which servers can be scoped, and their allowlists: the choices in the Scope section.
  useEffect(() => {
    if (isGhost) return;
    let on = true;
    client
      .sources()
      .then((a) => on && setSources(scopedSources(a)))
      .catch(() => on && setSources([]));
    return () => {
      on = false;
    };
  }, [client, isGhost, conversationsVersion]);

  // ---------------------------------------------------------------- derived
  const dirty = useMemo(() => (draft && base && detail ? dirtyKeys(draft, base, detail.is_ghost) : []), [draft, base, detail]);
  const voice = draft ? (voices.find((v) => v.id === draft.voice) ?? parseVoice(draft.voice)) : null;
  const pickable = useMemo(() => (draft && !voices.some((v) => v.id === draft.voice) ? [...voices, parseVoice(draft.voice)] : voices), [voices, draft]);
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

  const switchTo = async (target: string) => {
    const to = target === slug ? name : lineName(target);
    setActing("switch");
    try {
      const r = await client.switchConversation(target);
      if (r.ok === false) setNotice({ title: `Couldn’t switch to ${to}`, text: r.detail ?? "Ghost didn’t take the switch. Try again, or say it." });
      else toast({ title: `Patched through to ${to}`, description: target === "ghost" ? "You’re back on the main line." : `The session is on the ${to} line now.` });
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
      description: `Ghost forgets ${name}’s memory, pinned notes and transcript. The name, voice and persona stay.`,
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

  // Settle: announce-level, nothing destroyed (ADR 0019). Unsaved edits can't be kept on a
  // read-only line, so they are dropped with the settle; the dialog says so when there are any.
  const settle = async () => {
    const ok = await confirm({
      title: `Settle ${name}?`,
      description: `${name} leaves voice routing and the list of lines you can patch through to. Its transcript, memory and notes stay here to read, and Unsettle brings it back.${dirty.length ? " Your unsaved changes here are dropped." : ""}`,
      confirmLabel: `Settle ${name}`,
    });
    if (!ok) return;
    setActing("settle");
    try {
      // The route answers with an Ack, so the line is read back; the conversation event refreshes the lists.
      await settleLine(client, slug);
      drafts.delete(slug);
      baseRef.current = null;
      draftRef.current = null;
      await load();
      toast({ title: `Settled ${name}`, description: "It stays readable here and is out of voice routing. Unsettle brings it back." });
    } catch (e) {
      fail(`Couldn’t settle ${name}`, e);
    } finally {
      setActing(null);
    }
  };

  const unsettle = async () => {
    setActing("unsettle");
    try {
      await unsettleLine(client, slug);
      await load();
      toast({ title: `${name} is back`, description: `Say ${quoted(`patch me to ${name.toLowerCase()}`)} to reach it again.` });
    } catch (e) {
      fail(`Couldn’t bring ${name} back`, e);
    } finally {
      setActing(null);
    }
  };

  const remove = async () => {
    const ok = await confirm({
      title: `Delete ${name}?`,
      description: `The line goes, with its persona, memory, pinned notes and transcript.`,
      confirmLabel: `Delete ${name}`,
      tone: "danger",
    });
    if (!ok) return;
    try {
      await client.deleteConversation(slug);
      drafts.delete(slug);
      toast({ title: `Deleted ${name}` });
      navigate("/lines");
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
  // The destructive dialogs open only with the mock, and never over a line Ghost is on.
  const opened = useRef(false);
  useEffect(() => {
    if (!detail || opened.current) return;
    opened.current = true;
    const o = readParam("open", slug);
    if (o === "voice") setPicker(true);
    if (import.meta.env.DEV && design && !live) {
      if (o === "reset") void reset();
      if (o === "delete" && !isGhost) void remove();
      if (o === "settle" && !isGhost && !readOnly) void settle();
    }
    if (design && readParam("dirty") && draftRef.current) {
      setDraft({ persona: `${draftRef.current.persona}\nRead amounts in dollars, never cents.`.trim(), voice: "bm_lewis" });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail]);

  // A line just created opens with focus in its persona (set by the index before navigating).
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
    designRetry("line");
    await load();
    setRetrying(false);
  };

  // ---------------------------------------------------------------- render
  if (missing) {
    return (
      <Page width="default" title="Line not found" bare>
        <EmptyState
          size="page"
          icon={<SearchX />}
          title={`There’s no line called ${quoted(slug)}`}
          description="It may have been deleted, or the link has a typo."
          actions={<Button href="#/lines">See all lines</Button>}
        />
      </Page>
    );
  }

  const meta: string[] = [];
  if (detail) {
    if (isGhost) meta.push("Main line");
    if (settled) meta.push(`settled ${relativeTime(settled)}`);
    if (!live) meta.push(detail.last_active_at ? `last used ${relativeTime(detail.last_active_at)}` : "not used yet");
    if (detail.turn_count) {
      meta.push(plural(detail.turn_count, "turn"));
      meta.push(detail.compactions ? `compacted ${times(detail.compactions)}` : "never compacted");
    }
  }
  // The page status is the line's own state while it is live, in the sidebar's words
  // ("Waiting on your okay" in amber while a change waits here, else "Live · listening"), and
  // Ghost's state otherwise, like every other screen. Either way it stays in the compact bar
  // when the header scrolls away. The description keeps the line's facts.
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

  // Reset and delete need the session elsewhere. On the live line the menu says so and offers
  // the way out (back to Ghost). Every line, Ghost included, keeps them behind the same More
  // menu; Ghost has no Delete, and while Ghost is live its Reset is disabled with the reason.
  // The conversation is the page; its settings (voice and persona, memory, history) live
  // behind the same More menu as the actions (the operator's call, 2026-10-20).
  const mark = (keys: DraftKey[]) => (dirty.some((k) => keys.includes(k)) ? " (unsaved)" : "");
  const settingsItems: MenuItem[] = [
    { heading: "Settings" },
    { label: `Voice and persona${mark(PERSONA_KEYS)}`, icon: <SlidersHorizontal />, onSelect: () => setTab("persona") },
    { label: `Memory${mark(MEMORY_KEYS)}`, icon: <Brain />, onSelect: () => setTab("memory") },
    { label: "History", icon: <Clock />, onSelect: () => setTab("history") },
    "separator",
  ];
  const menu = detail && (
    <Menu
      align="end"
      trigger={<IconButton label={`More for ${name}`} icon={<MoreHorizontal />} variant="secondary" disabled={!online} />}
      items={[
        ...settingsItems,
        ...((isGhost
          ? [live ? { label: "Reset Ghost…", icon: <RotateCcw />, onSelect: () => void reset(), disabled: true, reason: "Ghost is on this line" } : { label: "Reset Ghost…", icon: <RotateCcw />, onSelect: () => void reset() }]
          : readOnly
            ? [
                { label: `Unsettle ${name}`, icon: <ArchiveRestore />, onSelect: () => void unsettle() },
                "separator",
                { label: `Delete ${name}…`, icon: <Trash2 />, danger: true, onSelect: () => void remove() },
              ]
          : live
            ? [
                { label: "Switch to Ghost", icon: <ArrowLeftRight />, onSelect: () => void switchTo("ghost") },
                "separator",
                // Not red while it can't be chosen: a red label reads as available.
                { label: `Settle ${name}…`, icon: <Archive />, onSelect: () => void settle(), disabled: true, reason: "Ghost is on this line" },
                { label: `Reset ${name}…`, icon: <RotateCcw />, onSelect: () => void reset(), disabled: true, reason: "Ghost is on this line" },
                { label: `Delete ${name}…`, icon: <Trash2 />, onSelect: () => void remove(), disabled: true, reason: "Ghost is on this line" },
              ]
            : [
                { label: `Settle ${name}…`, icon: <Archive />, onSelect: () => void settle() },
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
      {inSession && !live && !readOnly && (
        <Button variant="secondary" leadingIcon={<ArrowLeftRight />} loading={acting === "switch"} disabled={!online} onClick={() => void switchTo(slug)}>
          Switch to {name}
        </Button>
      )}
      {/* On a settled line, bringing it back is the one thing to do here. */}
      {readOnly && (
        <Button variant="primary" leadingIcon={<ArchiveRestore />} loading={acting === "unsettle"} disabled={!online} onClick={() => void unsettle()}>
          Unsettle
        </Button>
      )}
      {live && (
        <Button variant="secondary" leadingIcon={<Layers />} loading={acting === "compact"} disabled={!online} onClick={() => void compact()}>
          Compact now
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
    ) : undefined;

  return (
    <Page
      width="default"
      className="conv-page"
      title={name}
      leading={<LineGlyph slug={slug} name={name} size="xl" />}
      crumb={
        <Link to="/lines">
          <ChevronLeft size={14} aria-hidden />
          Lines
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

      {settled && (
        <p className="conv-settled t-small">
          <Archive size={14} aria-hidden />
          <span>
            Settled {relativeTime(settled)}. Read-only until you bring it back: {quoted(`patch me to ${name.toLowerCase()}`)} does nothing, and nothing here can change.
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
                Ghost didn’t send this line. Try again in a moment.
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
                  <>Ghost is the main line: every wake starts here, and {quoted("back to Ghost")} returns here. Its persona is in your words and goes into the prompt as written; Ghost never rewrites it.</>
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

            <Section
              id="conv-scope"
              title="Scope"
              description={
                isGhost ? (
                  "Ghost is the main line and sees everything: every repository and anything else a server allows."
                ) : (
                  <>What this line may read and change. Outside its scope, Ghost says it belongs to another line; Settings decides whether it answers read-only or patches you through. Ghost never writes outside it.</>
                )
              }
            >
              {isGhost ? (
                <Card>
                  <p className="conv-scope__all t-body">Ghost sees everything.</p>
                </Card>
              ) : (
                <ScopePicker sources={sources} value={draft.scope} onChange={(scope) => setDraft({ scope })} disabled={readOnly} idPrefix="conv-scope" />
              )}
            </Section>

            <Section id="conv-voice" title="Voice">
              <Card list>
                <div className="conv-voice">
                  <IconButton
                    variant="secondary"
                    className="conv-voice__play"
                    label={hearing === draft.voice ? `Playing ${voice ? voiceTitle(voice, voices) : draft.voice}` : `Hear ${voice ? voiceTitle(voice, voices) : draft.voice}`}
                    icon={hearing === draft.voice ? <PlayingBars /> : <Play />}
                    disabled={!!hearBlocked || (hearing != null && hearing !== draft.voice)}
                    onClick={() => void hear(draft.voice)}
                  />
                  <div className="conv-voice__text">
                    <p className="t-medium">{voice && voiceTitle(voice, voices)}</p>
                    <p className="t-small t-2" aria-live="polite">
                      {hearing === draft.voice && !picker ? (
                        <span className="conv-playing">Playing on your speakers…</span>
                      ) : (
                        <>
                          {voice && voiceKind(voice)}
                          <span className="t-3"> · </span>
                          <span className="t-mono t-3">{draft.voice}</span>
                        </>
                      )}
                    </p>
                  </div>
                  <Button variant="secondary" disabled={readOnly} onClick={() => setPicker(true)}>
                    Change voice
                  </Button>
                </div>
                <Field layout="inline" label="Speed" help="1.00× is the voice’s natural pace." className="conv-cell">
                  <Slider min={0.5} max={2} step={0.05} value={draft.speed} disabled={readOnly} format={(v) => `${v.toFixed(2)}×`} onChange={(v) => setDraft({ speed: v })} />
                </Field>
              </Card>
              {hearBlocked && <p className="t-small t-3">{hearBlocked}</p>}
            </Section>

            {!isGhost && (
              <Section id="conv-name" title="How you reach it" description={<>What you say in a session: {quoted(`patch me to ${(draft.name.trim() || name).toLowerCase()}`)}.</>}>
                <Card list>
                  <Field layout="inline" label="Name" className="conv-cell" error={draft.name.trim() ? undefined : "A line needs a name."}>
                    <Input value={draft.name} placeholder={name} disabled={readOnly} onChange={(e) => setDraft({ name: e.target.value })} />
                  </Field>
                  <Field layout="inline" label="Also answers to" help="Other names, separated by commas." className="conv-cell">
                    <Input value={draft.aliases} placeholder={aliasExample(draft.name || name)} disabled={readOnly} onChange={(e) => setDraft({ aliases: e.target.value })} />
                  </Field>
                </Card>
              </Section>
            )}
          </>
        ) : tab === "memory" ? (
          <>
            <Section
              id="conv-memory"
              title="Memory"
              description={
                readOnly
                  ? `The summary this line ended with. Start a new line from it to carry it on.${lastVersion ? ` Updated ${relativeTime(lastVersion.at)}.` : ""}`
                  : `The running summary Ghost keeps for this line. Compacting rewrites it; your edits are folded into the next version.${lastVersion ? ` Updated ${relativeTime(lastVersion.at)}.` : ""}`
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
                      newLineSeed.slug = slug;
                      navigate("/lines");
                    }}
                  >
                    Start a line from this memory
                  </Button>
                </div>
              )}
            </Section>
            <Section id="conv-notes" title="Pinned notes" description={<>Kept word for word and never summarized. Say {quoted("remember this")} in a session, or pin one here.</>}>
              <Notes notes={draft.notes} saved={base?.notes ?? []} onChange={(notes) => setDraft({ notes })} onPin={pin} pinning={pinning} disabled={!online || readOnly} />
            </Section>
            <p className="conv-files t-small t-3">
              Persona, memory and notes are files in <span className="t-mono">{detail.dir}</span>
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
          value={draft.voice}
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
