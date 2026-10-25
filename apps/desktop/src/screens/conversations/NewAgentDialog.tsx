/**
 * Owner: conversations. Hire an agent (ADR 0024): a name (what you say after "transfer me
 * to"), the department it works in, its job in a line, other names it answers to, an optional
 * persona in the operator's own words, and optionally a retired agent whose memory becomes
 * its starting memory. It inherits the department's voice until given one of its own.
 */
import type { ConversationDetail, ConversationView, DepartmentView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import { Button, Dialog, Field, Input, Select, Textarea, agentName, errorText, quoted, relativeTime, useLive } from "../../ui";
import { splitAliases } from "./draft";
import { aliasExample, PERSONA_EXAMPLE } from "./examples";
import { type AgentCreate, departmentTitle, retiredAt } from "./threads";

const NAME_EXAMPLES = ["Brand", "Social", "Vendors", "Hiring", "Release train", "Security"];

export interface NewAgentDialogProps {
  open: boolean;
  onClose: () => void;
  /** Names already taken, lowercased. */
  taken: string[];
  /** Retired agents a new one can start from (ADR 0024). */
  retired?: ConversationView[];
  /** The departments an agent can be hired into: Ghost first. */
  departments?: DepartmentView[];
  /** The department preselected when the dialog opens (a department page's New agent). */
  department?: string | null;
  /** A retired agent preselected as the starting memory, once, when the dialog opens. */
  seed?: string | null;
  onCreated: (d: ConversationDetail) => void;
}

export function NewAgentDialog({ open, onClose, taken, retired = [], departments = [], department: preset = null, seed = null, onCreated }: NewAgentDialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [name, setName] = useState("");
  const [aliases, setAliases] = useState("");
  const [job, setJob] = useState("");
  const [persona, setPersona] = useState("");
  const [seedFrom, setSeedFrom] = useState("");
  const [department, setDepartment] = useState("ghost");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setName("");
      setAliases("");
      setJob("");
      setPersona("");
      setSeedFrom("");
      setDepartment(preset ?? "ghost");
      setError(null);
      seeded.current = false;
      // showModal() focuses the close button; the name is where the operator starts.
      const id = requestAnimationFrame(() => nameRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [open, preset]);

  // Starting from a retired agent: once, when it is known (the list may land after the dialog
  // opens from the URL); it is hired into the same department unless the operator picks another.
  const seeded = useRef(false);
  useEffect(() => {
    if (!open || !seed || seeded.current) return;
    const from = retired.find((l) => l.slug === seed);
    if (!from) return;
    seeded.current = true;
    setSeedFrom(from.slug);
    if (!preset) setDepartment(from.department || "ghost");
  }, [open, seed, retired, preset]);

  const trimmed = name.trim();
  const clash = trimmed && taken.includes(trimmed.toLowerCase()) ? `There’s already an agent called ${trimmed}. Pick another name.` : null;
  const shown = clash ?? error;
  const deptTitle = departmentTitle(department, departments);

  const submit = async () => {
    if (!trimmed || clash || busy) return;
    setBusy(true);
    setError(null);
    try {
      const body: AgentCreate = {
        name: trimmed,
        aliases: splitAliases(aliases),
        department: department === "ghost" ? null : department,
        job: job.trim(),
        voice: null,
        speed: null,
        persona,
        seed_from: seedFrom || null,
      };
      onCreated(await client.createConversation(body));
    } catch (e) {
      setError(`Ghost couldn’t hire this agent: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // An example name that isn't already an agent, so the placeholder never reads as a clash.
  const example = NAME_EXAMPLES.find((n) => !taken.includes(n.toLowerCase())) ?? "Brand";
  const spoken = (trimmed || example).toLowerCase();

  // Retired agents from the same department first: those are the ones this one most likely continues.
  const seeds = [...retired].sort((a, b) => {
    const sa = a.department === department ? 0 : 1;
    const sb = b.department === department ? 0 : 1;
    return sa - sb || (retiredAt(b) ?? "").localeCompare(retiredAt(a) ?? "");
  });
  const seedAgent = retired.find((l) => l.slug === seedFrom);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="New agent"
      description="An agent has its own persona, job and memory, and works in a department: its servers, rules and skills. Transfer to it in any session."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="conv-new-agent" loading={busy} disabled={!trimmed || !!clash || !online}>
            Hire {trimmed || "agent"}
          </Button>
        </>
      }
    >
      <form
        id="conv-new-agent"
        className="conv-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Name" error={shown} help={<>You’ll say {quoted(`transfer me to ${spoken}`)}.</>}>
          <Input ref={nameRef} value={name} placeholder={example} maxLength={60} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field
          label="Department"
          help={department === "ghost" ? <>Works with Ghost’s own servers and skills, like Ghost.</> : <>Works with {deptTitle}’s servers, skills and rules, and reads what {deptTitle} knows. {quoted(`transfer me to ${deptTitle.toLowerCase()}`)} still lands on the department’s own agent.</>}
        >
          <Select value={department} onChange={(e) => setDepartment(e.target.value)} options={[{ value: "ghost", label: "Ghost" }, ...departments.filter((d) => !d.is_ghost).map((d) => ({ value: d.slug, label: d.title }))]} />
        </Field>
        <Field label="Job" optional help="What it is for, in a line: brand design, the Q3 audit, social media.">
          <Input value={job} placeholder="brand design" maxLength={200} onChange={(e) => setJob(e.target.value)} />
        </Field>
        <Field label="Also answers to" optional help="Other names for it, separated by commas.">
          <Input value={aliases} placeholder={aliasExample(trimmed || example)} onChange={(e) => setAliases(e.target.value)} />
        </Field>
        <Field label="Persona" optional help="In your words. It goes into the prompt exactly as written. You can add it later.">
          <Textarea rows={4} value={persona} placeholder={PERSONA_EXAMPLE} onChange={(e) => setPersona(e.target.value)} />
        </Field>
        {retired.length > 0 && (
          <Field
            label="Start from a retired agent’s memory"
            optional
            help={
              seedAgent
                ? `${agentName(seedAgent.slug, seedAgent.name)}’s memory becomes this agent’s first memory, a handoff you can edit on its Memory tab.`
                : "Its memory becomes this agent’s first memory, a handoff you can edit before the first session."
            }
          >
            <Select
              value={seedFrom}
              onChange={(e) => setSeedFrom(e.target.value)}
              options={[
                { value: "", label: "Start with an empty memory" },
                ...seeds.map((l) => {
                  const when = retiredAt(l);
                  return { value: l.slug, label: `${agentName(l.slug, l.name)}${when ? ` · retired ${relativeTime(when)}` : ""}${l.department === department ? ` · ${deptTitle}` : ""}` };
                }),
              ]}
            />
          </Field>
        )}
      </form>
    </Dialog>
  );
}
