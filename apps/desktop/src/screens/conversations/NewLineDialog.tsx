/**
 * Owner: conversations. Start a new line: a name (what you say after "patch me to"),
 * other names it answers to, an optional persona in the operator's own words, where it sits
 * (a department of its own, with toolkits, or a topic line inside one, ADR 0023), what it
 * may touch (ADR 0019), and optionally a settled line whose memory becomes its starting
 * memory. Or start a whole department from a template, which brings its toolkits, skills
 * and persona along in one step.
 */
import type { ConversationDetail, ConversationView, TemplateView, ToolkitView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import { Button, Card, Checkbox, Dialog, Field, Input, Select, Skeleton, Textarea, errorText, lineName, quoted, relativeTime, useLive } from "../../ui";
import { cleanScope, splitAliases } from "./draft";
import { aliasExample, PERSONA_EXAMPLE } from "./examples";
import { ScopePicker } from "./ScopePicker";
import { type Scope, type ScopedSource, type ThreadCreate, narrowSources, sameScope, scopeOf, scopedSources, settledAt, toolkitsOf } from "./threads";

const NAME_EXAMPLES = ["Vendors", "Legal", "Hiring", "Release train", "Security", "Support"];

/** "a", "a and b", "a, b and c". */
const words = (xs: string[]) => (xs.length <= 1 ? xs.join("") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`);

export interface NewLineDialogProps {
  open: boolean;
  onClose: () => void;
  /** Names already taken, lowercased. */
  taken: string[];
  /** Settled lines a new one can start from (ADR 0019). */
  settled?: ConversationView[];
  /** Departments a new line can sit inside (ADR 0023): the live lines that are one. */
  departments?: ConversationView[];
  /** A settled line preselected as the starting memory, once, when the dialog opens. */
  seed?: string | null;
  onCreated: (d: ConversationDetail) => void;
}

export function NewLineDialog({ open, onClose, taken, settled = [], departments = [], seed = null, onCreated }: NewLineDialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [name, setName] = useState("");
  const [aliases, setAliases] = useState("");
  const [persona, setPersona] = useState("");
  const [scope, setScope] = useState<Scope>({});
  const [seedFrom, setSeedFrom] = useState("");
  const [department, setDepartment] = useState("");
  const [toolkits, setToolkits] = useState<string[]>([]);
  const [template, setTemplate] = useState("");
  const [sources, setSources] = useState<ScopedSource[] | null>(null);
  const [toolkitList, setToolkitList] = useState<ToolkitView[] | null>(null);
  const [templates, setTemplates] = useState<TemplateView[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setName("");
      setAliases("");
      setPersona("");
      setSeedFrom("");
      setScope({});
      setDepartment("");
      setToolkits([]);
      setTemplate("");
      setError(null);
      seeded.current = false;
      // showModal() focuses the close button; the name is where the operator starts.
      const id = requestAnimationFrame(() => nameRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [open]);

  // Starting from a settled line: once, when it is known (the list may land after the dialog
  // opens from the URL), its scope comes along too, so the new line sees the same things.
  const seeded = useRef(false);
  useEffect(() => {
    if (!open || !seed || seeded.current) return;
    const from = settled.find((l) => l.slug === seed);
    if (!from) return;
    seeded.current = true;
    setSeedFrom(from.slug);
    setScope(scopeOf(from));
  }, [open, seed, settled]);

  // Which servers can be scoped, the toolkits the file declares and the templates on hand,
  // read when the dialog opens; a failure just hides that picker's items.
  useEffect(() => {
    if (!open) return;
    let on = true;
    client
      .sources()
      .then((a) => on && setSources(scopedSources(a)))
      .catch(() => on && setSources([]));
    client
      .toolkits()
      .then((t) => on && setToolkitList(t))
      .catch(() => on && setToolkitList([]));
    client
      .templates()
      .then((t) => on && setTemplates(t))
      .catch(() => on && setTemplates([]));
    return () => {
      on = false;
    };
  }, [open, client]);

  const tpl = templates?.find((t) => t.name === template) ?? null;
  const dept = departments.find((d) => d.slug === department) ?? null;
  const trimmed = name.trim();
  // From a template the name defaults to its title; a different one renames the department.
  const finalName = tpl ? trimmed || tpl.title : trimmed;
  const clash = finalName && taken.includes(finalName.toLowerCase()) ? `There’s already a line called ${finalName}. Pick another name.` : null;
  const shown = clash ?? error;

  const submit = async () => {
    if (!finalName || clash || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (tpl) {
        onCreated(await client.applyTemplate(tpl.name, trimmed && trimmed !== tpl.title ? trimmed : null));
        return;
      }
      const body: ThreadCreate = {
        name: finalName,
        aliases: splitAliases(aliases),
        voice: null,
        speed: null,
        persona,
        scope: cleanScope(scope),
        seed_from: seedFrom || null,
        department: department || null,
        // Inside a department the line reaches what the department reaches (ADR 0023).
        toolkits: department ? [] : toolkits,
      };
      onCreated(await client.createConversation(body));
    } catch (e) {
      setError(`Ghost couldn’t start this line: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // An example name that isn't already a line, so the placeholder never reads as a clash.
  const example = NAME_EXAMPLES.find((n) => !taken.includes(n.toLowerCase())) ?? "Vendors";
  const spoken = (finalName || example).toLowerCase();

  // Settled lines with the same scope first: those are the ones this line most likely continues.
  const seeds = [...settled].sort((a, b) => {
    const sa = sameScope(scopeOf(a), scope) ? 0 : 1;
    const sb = sameScope(scopeOf(b), scope) ? 0 : 1;
    return sa - sb || (settledAt(b) ?? "").localeCompare(settledAt(a) ?? "");
  });
  const seedLine = settled.find((l) => l.slug === seedFrom);
  // A topic line's scope can only name what its department's toolkits grant (where that can be read off them).
  const shownSources = sources && dept ? narrowSources(sources, toolkitsOf(dept), toolkitList) : sources;
  const deptReach = dept ? (toolkitsOf(dept).length ? `what its ${toolkitsOf(dept).length === 1 ? "toolkit" : "toolkits"} ${words(toolkitsOf(dept))} ${toolkitsOf(dept).length === 1 ? "grants" : "grant"}` : "everything, like Ghost") : "";

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="New line"
      description="A line has its own voice, persona and memory. Patch through to it in any session."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="conv-new-line" loading={busy} disabled={!finalName || !!clash || !online}>
            Start {finalName || "line"}
          </Button>
        </>
      }
    >
      <form
        id="conv-new-line"
        className="conv-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        {templates && templates.length > 0 && (
          <Field
            label="Start from a template"
            optional
            help={
              tpl ? (
                <>
                  <span data-user-content>{tpl.description || `${tpl.title}, with its persona, toolkits and skills.`}</span>
                  {" "}Brings {[tpl.toolkits.length ? `${tpl.toolkits.length === 1 ? "the toolkit" : "the toolkits"} ${words(tpl.toolkits)}` : null, tpl.skills.length ? `${tpl.skills.length === 1 ? "the skill" : "the skills"} ${words(tpl.skills)}` : null].filter(Boolean).join(" and ") || "its persona"} into your setup.
                  {tpl.needs.length > 0 && <span className="conv-form__warn"> Needs {words(tpl.needs)} declared in your policy file first; until then that part of it can’t work.</span>}
                </>
              ) : (
                "A department in one step: its persona, toolkits and skills come with it. Templates Ghost ships, and ones you’ve saved."
              )
            }
          >
            <Select
              value={template}
              onChange={(e) => setTemplate(e.target.value)}
              options={[{ value: "", label: "No template" }, ...templates.map((t) => ({ value: t.name, label: `${t.title}${t.source === "shipped" ? "" : " · yours"}` }))]}
            />
          </Field>
        )}
        <Field label="Name" error={shown} help={<>You’ll say {quoted(`patch me to ${spoken}`)}.</>}>
          <Input ref={nameRef} value={name} placeholder={tpl ? tpl.title : example} maxLength={60} onChange={(e) => setName(e.target.value)} />
        </Field>
        {!tpl && (
          <>
            <Field label="Also answers to" optional help="Other names for it, separated by commas.">
              <Input value={aliases} placeholder={aliasExample(finalName || example)} onChange={(e) => setAliases(e.target.value)} />
            </Field>
            <Field label="Persona" optional help="In your words. It goes into the prompt exactly as written. You can add it later.">
              <Textarea rows={4} value={persona} placeholder={PERSONA_EXAMPLE} onChange={(e) => setPersona(e.target.value)} />
            </Field>
            {departments.length > 0 && (
              <Field
                label="In department"
                optional
                help={dept ? <>A line in {lineName(dept.slug, dept.name)}, for one topic. It reaches {deptReach}, and {quoted(`transfer me to ${lineName(dept.slug, dept.name).toLowerCase()}`)} still lands on the department’s own line.</> : "On its own, it is a department: give it toolkits below. Inside one, it is a topic line and reaches what the department reaches."}
              >
                <Select value={department} onChange={(e) => setDepartment(e.target.value)} options={[{ value: "", label: "Its own department" }, ...departments.map((d) => ({ value: d.slug, label: lineName(d.slug, d.name) }))]} />
              </Field>
            )}
            {!dept && (toolkitList === null || toolkitList.length > 0) && (
              <div className="conv-form__group">
                <p className="conv-form__label">
                  Toolkits <span className="conv-form__optional">Optional</span>
                </p>
                <ToolkitPicker toolkits={toolkitList} value={toolkits} onChange={setToolkits} idPrefix="conv-new-toolkit" />
              </div>
            )}
            {(shownSources === null || shownSources.length > 0) && (
              <div className="conv-form__group">
                <p className="conv-form__label">
                  Scope <span className="conv-form__optional">Optional</span>
                </p>
                <ScopePicker sources={shownSources} value={scope} onChange={setScope} idPrefix="conv-new-scope" />
              </div>
            )}
            {settled.length > 0 && (
              <Field
                label="Start from a settled line’s memory"
                optional
                help={
                  seedLine
                    ? `${lineName(seedLine.slug, seedLine.name)}’s memory becomes this line’s first memory, a handoff you can edit on its Memory tab.`
                    : "Its memory becomes this line’s first memory, a handoff you can edit before the first session."
                }
              >
                <Select
                  value={seedFrom}
                  onChange={(e) => setSeedFrom(e.target.value)}
                  options={[
                    { value: "", label: "Start with an empty memory" },
                    ...seeds.map((l) => {
                      const when = settledAt(l);
                      const same = sameScope(scopeOf(l), scope);
                      return { value: l.slug, label: `${lineName(l.slug, l.name)}${when ? ` · settled ${relativeTime(when)}` : ""}${same && Object.keys(scope).length ? " · same scope" : ""}` };
                    }),
                  ]}
                />
              </Field>
            )}
          </>
        )}
      </form>
    </Dialog>
  );
}

/**
 * A department's toolkits (ADR 0023): a checklist of the ones the policy file declares. Nothing
 * checked means the department reaches everything, like Ghost, so the help line says so.
 * Checks apply on Save (or Start), so these are Checkboxes.
 */
export function ToolkitPicker({ toolkits, value, onChange, disabled, idPrefix }: { toolkits: ToolkitView[] | null; value: string[]; onChange: (names: string[]) => void; disabled?: boolean; idPrefix: string }) {
  if (!toolkits) {
    return (
      <Card list aria-busy="true" aria-label="Loading toolkits">
        {[0, 1].map((i) => (
          <div key={i} className="conv-scope-skel">
            <Skeleton width={16} height={16} radius="var(--radius-xs)" />
            <Skeleton width={i ? 140 : 120} height={12} />
          </div>
        ))}
      </Card>
    );
  }
  // Toolkits the line names that the file no longer declares stay visible until unchecked.
  const gone = value.filter((n) => !toolkits.some((t) => t.name === n));
  if (!toolkits.length && !gone.length) {
    return <p className="t-small t-2">No toolkits yet: this department reaches everything, like Ghost. Design one on the Toolkits screen to narrow it.</p>;
  }
  const toggle = (name: string, on: boolean) => onChange(on ? [...value.filter((n) => n !== name), name] : value.filter((n) => n !== name));
  const reach = (t: ToolkitView) => [t.grants.length ? `${t.grants.length} ${t.grants.length === 1 ? "server" : "servers"}` : null, t.skills.length ? `${t.skills.length} ${t.skills.length === 1 ? "skill" : "skills"}` : null].filter(Boolean).join(" · ") || "reaches nothing yet";
  return (
    <div className="conv-scope__source">
      <Card list>
        {toolkits.map((t) => (
          <div key={t.name} className="conv-scope__item">
            <Checkbox
              id={`${idPrefix}-${t.name}`}
              checked={value.includes(t.name)}
              disabled={disabled}
              onChange={(on) => toggle(t.name, on)}
              label={
                <>
                  {t.name.replace(/[_-]+/g, " ").replace(/^\w/, (c) => c.toUpperCase())} <span className="t-mono t-3">{t.name}</span>
                </>
              }
              description={
                <>
                  {t.description && <span data-user-content>{t.description} </span>}
                  <span className="t-3">{reach(t)}</span>
                  {t.problems.length > 0 && <span className="t-warn"> · {t.problems.length === 1 ? "one problem" : `${t.problems.length} problems`} on the Toolkits screen</span>}
                </>
              }
            />
          </div>
        ))}
        {gone.map((n) => (
          <div key={n} className="conv-scope__item">
            <Checkbox id={`${idPrefix}-${n}`} checked disabled={disabled} onChange={() => toggle(n, false)} label={<span className="t-mono">{n}</span>} description="Not in the policy file anymore. Uncheck it to drop it." />
          </div>
        ))}
      </Card>
      <p className="conv-scope__help t-small t-3">
        {value.length === 0 ? "Nothing checked: this department reaches everything, like Ghost." : value.length === 1 ? `Only what ${value[0]} grants, in this department and its lines.` : `What ${words(value)} grant, together, in this department and its lines.`}
      </p>
    </div>
  );
}
