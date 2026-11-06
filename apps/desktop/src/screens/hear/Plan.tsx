/**
 * Owner: hear. The take list (GET/PUT/DELETE /hear/plan): its blocks (a name, the condition,
 * a title and what to do, across the room, with music) and their takes (an id, the kind, the
 * line, tags and the names Ghost must hear; a silent take's instruction and length). Add,
 * remove and reorder both; Save through the SaveBar (Ctrl S), and the daemon says why when the
 * list doesn't hold together. A take's id never changes once recorded: add new takes with new
 * ids. The draft lives with the screen, so moving between tabs keeps it.
 */
import type { HearBlock, HearKind, HearPlan, HearPlanTake, HearPlanView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ArrowDown, ArrowUp, Plus, RotateCcw, Trash2 } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Badge, Button, Card, Checkbox, Field, IconButton, Input, Notice, SaveBar, Select, Skeleton, Textarea, useConfirm, useLive, useToast } from "../../ui";
import { freeBlockName, freeTakeId, moved, refusal, splitList } from "./model";

type DraftTake = HearPlanTake & { uid: number };
type DraftBlock = Omit<HearBlock, "takes"> & { uid: number; takes: DraftTake[] };
interface Draft {
  version: number;
  about: string;
  blocks: DraftBlock[];
}

let uids = 0;
const uid = () => ++uids;

function toDraft(p: HearPlan): Draft {
  return {
    version: p.version,
    about: p.about ?? "",
    blocks: p.blocks.map((b) => ({ ...b, uid: uid(), takes: b.takes.map((t) => ({ ...t, uid: uid() })) })),
  };
}

/** The plan as the daemon takes it: no editor ids, a silent take without a line, a spoken one without a length. */
function toPlan(d: Draft): HearPlan {
  return {
    version: d.version,
    about: d.about,
    blocks: d.blocks.map(({ uid: _u, takes, ...b }) => ({
      ...b,
      takes: takes.map(({ uid: _t, ...t }) =>
        t.kind === "silence" ? { ...t, text: "", seconds: t.seconds ?? 6, say: t.say ?? "(say nothing)" } : { ...t, say: null, seconds: null },
      ),
    })),
  };
}

const same = (a: HearPlan, b: HearPlan) => JSON.stringify(a) === JSON.stringify(b);
/** A plan as the editor would save it, so an untouched draft never reads as a change. */
const norm = (p: HearPlan) => toPlan(toDraft(p));

export interface PlanEditor {
  view: HearPlanView | null;
  draft: Draft | null;
  /** Bumps on Discard and Reset, so the fields typed as text start again from the draft. */
  generation: number;
  dirty: boolean;
  saving: boolean;
  saveError: string | null;
  update: (f: (d: Draft) => Draft) => void;
  save: () => Promise<void>;
  discard: () => void;
  reset: () => Promise<void>;
  confirmDialog: ReactNode;
}

export function usePlanEditor(view: HearPlanView | null, onSaved: (v: HearPlanView) => void, active: boolean): PlanEditor {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [generation, setGeneration] = useState(0);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // A fresh plan from the daemon starts the draft again, unless the operator is mid-edit.
  const base = useRef<HearPlan | null>(null);
  useEffect(() => {
    if (!view) return;
    const fresh = view.plan;
    setDraft((d) => (!d || !base.current || same(toPlan(d), norm(base.current)) ? toDraft(fresh) : d));
    base.current = fresh;
  }, [view]);

  const saved = useMemo(() => (view ? norm(view.plan) : null), [view]);
  const dirty = !!draft && !!saved && !same(toPlan(draft), saved);

  const update = useCallback((f: (d: Draft) => Draft) => {
    setSaveError(null);
    setDraft((d) => (d ? f(d) : d));
  }, []);

  const save = useCallback(async () => {
    if (!draft || !dirty || saving || !online) return;
    setSaving(true);
    setSaveError(null);
    try {
      const v = await client.saveHearPlan(toPlan(draft));
      base.current = v.plan;
      setDraft(toDraft(v.plan));
      setGeneration((g) => g + 1);
      onSaved(v);
      toast({ title: "Saved", description: "The next take you record reads the new take list." });
    } catch (e) {
      setSaveError(refusal(e));
    } finally {
      setSaving(false);
    }
  }, [client, draft, dirty, saving, online, onSaved, toast]);

  const discard = useCallback(() => {
    if (!view) return;
    setDraft(toDraft(view.plan));
    setGeneration((g) => g + 1);
    setSaveError(null);
  }, [view]);

  const reset = useCallback(async () => {
    const ok = await confirm({
      title: "Reset to Ghost’s take list?",
      description: "Your take list is replaced by the one Ghost ships. Sessions already recorded keep their takes; a take whose id is no longer in the list is left out of the next score.",
      confirmLabel: "Reset the take list",
      tone: "danger",
    });
    if (!ok) return;
    try {
      const v = await client.resetHearPlan();
      base.current = v.plan;
      setDraft(toDraft(v.plan));
      setGeneration((g) => g + 1);
      onSaved(v);
      toast({ title: "Saved", description: "The take list is Ghost’s again." });
    } catch (e) {
      toast({ title: "Couldn’t reset the take list", description: refusal(e), tone: "danger" });
    }
  }, [client, confirm, onSaved, toast]);

  // Ctrl S saves, while the take list is the tab.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    if (!active) return;
    const on = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [active]);

  return { view, draft, generation, dirty, saving, saveError, update, save, discard, reset, confirmDialog: dialog };
}

export function PlanBar({ editor }: { editor: PlanEditor }) {
  const { online } = useLive();
  if (!editor.dirty) return null;
  return (
    <SaveBar
      variant={!online ? "offline" : editor.saveError ? "error" : "dirty"}
      title={editor.saveError ? "Couldn’t save" : "Unsaved changes"}
      detail={!online ? "they wait here until Ghost is back" : "the take list"}
      description={editor.saveError ?? undefined}
      busy={editor.saving}
      onSave={() => void editor.save()}
      onDiscard={editor.discard}
    />
  );
}

const KINDS: { value: HearKind; label: string }[] = [
  { value: "speech", label: "Request" },
  { value: "reply", label: "Reply" },
  { value: "silence", label: "Silent" },
];

export function PlanTab({ editor, error, onRetry }: { editor: PlanEditor; error: string | null; onRetry: () => void }) {
  const { online } = useLive();
  const { view, draft, update, generation } = editor;
  const count = useMemo(() => draft?.blocks.reduce((n, b) => n + b.takes.length, 0) ?? 0, [draft]);

  if (error && !view) {
    return (
      <Notice tone="danger" title="Couldn’t load the take list" action={<Button size="sm" onClick={onRetry}>Try again</Button>}>
        {error}
      </Notice>
    );
  }
  if (!view || !draft) {
    return (
      <div className="stack gap-4" aria-busy="true" aria-label="Loading">
        <Skeleton width={260} height={14} />
        {[0, 1].map((i) => (
          <Card key={i} padding="md">
            <div className="stack gap-3">
              <Skeleton width={120} height={14} />
              <Skeleton width="90%" height={10} />
              <Skeleton width="70%" height={10} />
            </div>
          </Card>
        ))}
      </div>
    );
  }

  const setBlock = (bi: number, patch: Partial<DraftBlock>) => update((d) => ({ ...d, blocks: d.blocks.map((b, i) => (i === bi ? { ...b, ...patch } : b)) }));
  const setTake = (bi: number, ti: number, patch: Partial<DraftTake>) =>
    update((d) => ({ ...d, blocks: d.blocks.map((b, i) => (i === bi ? { ...b, takes: b.takes.map((t, j) => (j === ti ? { ...t, ...patch } : t)) } : b)) }));
  const addTake = (bi: number) =>
    update((d) => {
      const b = d.blocks[bi];
      const id = freeTakeId(toPlan(d), b.block);
      const take: DraftTake = { uid: uid(), id, kind: "speech", text: "", tags: [], terms: [] };
      return { ...d, blocks: d.blocks.map((x, i) => (i === bi ? { ...x, takes: [...x.takes, take] } : x)) };
    });
  const addBlock = () =>
    update((d) => {
      const name = freeBlockName(toPlan(d));
      const block: DraftBlock = { uid: uid(), block: name, condition: "desk", title: "", instruction: "", away: false, music: false, takes: [] };
      block.takes = [{ uid: uid(), id: `${name}01`, kind: "speech", text: "", tags: [], terms: [] }];
      return { ...d, blocks: [...d.blocks, block] };
    });

  return (
    <div className="stack gap-6 hear-plan" key={generation}>
      <div className="hear-plan__head">
        <p className="t-body t-2">
          {view.edited ? <Badge tone="accent">Your take list</Badge> : <Badge>Ghost’s take list</Badge>} {draft.blocks.length} blocks, {count} takes. A pause in a line is written <span className="t-mono">...</span>; a take’s id stays the same once it is recorded, so add a new take rather than reusing an id.
        </p>
        <Button variant="danger-ghost" leadingIcon={<RotateCcw />} onClick={() => void editor.reset()} disabled={!online || !view.edited}>
          Reset to Ghost’s take list
        </Button>
      </div>

      {draft.blocks.map((b, bi) => (
        <Card key={b.uid} padding="lg" className="hear-plan__block">
          <div className="hear-plan__block-head">
            <h2 className="t-heading">
              Block {b.block || "?"}
              {b.title && <span className="t-2"> · {b.title}</span>}
            </h2>
            <div className="hear-plan__tools">
              <IconButton size="sm" variant="ghost" label={`Move block ${b.block} up`} icon={<ArrowUp />} disabled={bi === 0} onClick={() => update((d) => ({ ...d, blocks: moved(d.blocks, bi, -1) }))} />
              <IconButton size="sm" variant="ghost" label={`Move block ${b.block} down`} icon={<ArrowDown />} disabled={bi === draft.blocks.length - 1} onClick={() => update((d) => ({ ...d, blocks: moved(d.blocks, bi, 1) }))} />
              <IconButton size="sm" variant="ghost" label={`Remove block ${b.block}`} icon={<Trash2 />} disabled={draft.blocks.length === 1} onClick={() => update((d) => ({ ...d, blocks: d.blocks.filter((_, i) => i !== bi) }))} />
            </div>
          </div>
          <div className="hear-plan__block-fields">
            <Field label="Block">
              <Input mono value={b.block} maxLength={8} placeholder="A" onChange={(e) => setBlock(bi, { block: e.target.value.trim() })} />
            </Field>
            <Field label="Condition" help="What the score groups by.">
              <Input mono value={b.condition} placeholder="desk-pause" onChange={(e) => setBlock(bi, { condition: e.target.value })} />
            </Field>
            <Field label="Title" className="hear-plan__wide">
              <Input value={b.title ?? ""} placeholder="At the desk, a pause mid-sentence" onChange={(e) => setBlock(bi, { title: e.target.value })} />
            </Field>
            <Field label="What to do" className="hear-plan__full">
              <Textarea rows={2} value={b.instruction ?? ""} placeholder="Where the line shows a pause, stop for about a second, then carry on." onChange={(e) => setBlock(bi, { instruction: e.target.value })} />
            </Field>
            <div className="hear-plan__checks hear-plan__full">
              <Checkbox checked={!!b.away} onChange={(v) => setBlock(bi, { away: v })} label="Across the room" description="Takes start after a countdown and end on silence." />
              <Checkbox checked={!!b.music} onChange={(v) => setBlock(bi, { music: v })} label="With music" description="The screen asks for music before the block." />
            </div>
          </div>

          <ol className="hear-plan__takes" role="list">
            {b.takes.map((t, ti) => (
              <li key={t.uid} className="hear-plan__take">
                <div className="hear-plan__take-row">
                  <Input mono size="sm" value={t.id} maxLength={16} placeholder="A01" aria-label="Take id" className="hear-plan__id" onChange={(e) => setTake(bi, ti, { id: e.target.value.trim() })} />
                  <Select size="sm" value={t.kind} aria-label={`Kind of take ${t.id}`} className="hear-plan__kind" onChange={(e) => setTake(bi, ti, { kind: e.target.value as HearKind })} options={KINDS} />
                  {t.kind === "silence" ? (
                    <>
                      <Input size="sm" value={t.say ?? ""} placeholder="(sit still, say nothing)" aria-label={`What to do in take ${t.id}`} className="hear-plan__text" onChange={(e) => setTake(bi, ti, { say: e.target.value })} />
                      <Input
                        size="sm"
                        type="number"
                        min={1}
                        max={60}
                        step={1}
                        value={t.seconds ?? 6}
                        aria-label={`Seconds in take ${t.id}`}
                        className="hear-plan__secs"
                        trailing={<span className="t-small t-2">s</span>}
                        onChange={(e) => setTake(bi, ti, { seconds: Number(e.target.value) || null })}
                      />
                    </>
                  ) : (
                    <Input size="sm" value={t.text ?? ""} placeholder="Read me issue four oh three." aria-label={`Line of take ${t.id}`} className="hear-plan__text" onChange={(e) => setTake(bi, ti, { text: e.target.value })} />
                  )}
                  <div className="hear-plan__tools">
                    <IconButton size="sm" variant="ghost" label={`Move ${t.id} up`} icon={<ArrowUp />} disabled={ti === 0} onClick={() => setBlock(bi, { takes: moved(b.takes, ti, -1) })} />
                    <IconButton size="sm" variant="ghost" label={`Move ${t.id} down`} icon={<ArrowDown />} disabled={ti === b.takes.length - 1} onClick={() => setBlock(bi, { takes: moved(b.takes, ti, 1) })} />
                    <IconButton size="sm" variant="ghost" label={`Remove ${t.id}`} icon={<Trash2 />} onClick={() => setBlock(bi, { takes: b.takes.filter((_, j) => j !== ti) })} />
                  </div>
                </div>
                <div className="hear-plan__take-row hear-plan__take-row--meta">
                  <Input
                    size="sm"
                    defaultValue={(t.tags ?? []).join(", ")}
                    placeholder="Tags: numbers, pause"
                    aria-label={`Tags of take ${t.id}`}
                    className="hear-plan__list"
                    onChange={(e) => setTake(bi, ti, { tags: splitList(e.target.value) })}
                  />
                  {t.kind !== "silence" && (
                    <Input
                      size="sm"
                      defaultValue={(t.terms ?? []).join(", ")}
                      placeholder="Names Ghost must hear: Mobbin, GitHub"
                      aria-label={`Names Ghost must hear in take ${t.id}`}
                      className="hear-plan__list"
                      onChange={(e) => setTake(bi, ti, { terms: splitList(e.target.value) })}
                    />
                  )}
                </div>
              </li>
            ))}
          </ol>
          <div>
            <Button size="sm" variant="ghost" leadingIcon={<Plus />} onClick={() => addTake(bi)}>
              Add a take to block {b.block}
            </Button>
          </div>
        </Card>
      ))}
      <div>
        <Button variant="secondary" leadingIcon={<Plus />} onClick={addBlock}>
          Add a block
        </Button>
      </div>
      {editor.confirmDialog}
    </div>
  );
}
