/**
 * Owner: conversations. Start a new line: a name (what you say after "patch me to"),
 * other names it answers to, an optional persona in the operator's own words, what it may
 * touch (ADR 0019), and optionally a settled line whose memory becomes its starting memory.
 */
import type { ConversationDetail, ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import { Button, Dialog, Field, Input, Select, Textarea, errorText, lineName, quoted, relativeTime, useLive } from "../../ui";
import { cleanScope, splitAliases } from "./draft";
import { aliasExample, PERSONA_EXAMPLE } from "./examples";
import { ScopePicker } from "./ScopePicker";
import { type Scope, type ScopedAbility, type ThreadCreate, sameScope, scopeOf, scopedAbilities, settledAt } from "./threads";

const NAME_EXAMPLES = ["Vendors", "Legal", "Hiring", "Release train", "Security", "Support"];

export interface NewLineDialogProps {
  open: boolean;
  onClose: () => void;
  /** Names already taken, lowercased. */
  taken: string[];
  /** Settled lines a new one can start from (ADR 0019). */
  settled?: ConversationView[];
  /** A settled line preselected as the starting memory, once, when the dialog opens. */
  seed?: string | null;
  onCreated: (d: ConversationDetail) => void;
}

export function NewLineDialog({ open, onClose, taken, settled = [], seed = null, onCreated }: NewLineDialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [name, setName] = useState("");
  const [aliases, setAliases] = useState("");
  const [persona, setPersona] = useState("");
  const [scope, setScope] = useState<Scope>({});
  const [seedFrom, setSeedFrom] = useState("");
  const [abilities, setAbilities] = useState<ScopedAbility[] | null>(null);
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

  // Which abilities can be scoped, read when the dialog opens; a failure just hides the picker's items.
  useEffect(() => {
    if (!open) return;
    let on = true;
    client
      .abilities()
      .then((a) => on && setAbilities(scopedAbilities(a)))
      .catch(() => on && setAbilities([]));
    return () => {
      on = false;
    };
  }, [open, client]);

  const trimmed = name.trim();
  const clash = trimmed && taken.includes(trimmed.toLowerCase()) ? `There’s already a line called ${trimmed}. Pick another name.` : null;
  const shown = clash ?? error;

  const submit = async () => {
    if (!trimmed || clash || busy) return;
    setBusy(true);
    setError(null);
    try {
      const body: ThreadCreate = { name: trimmed, aliases: splitAliases(aliases), voice: null, speed: null, persona, scope: cleanScope(scope), seed_from: seedFrom || null };
      const d = await client.createConversation(body);
      onCreated(d);
    } catch (e) {
      setError(`Ghost couldn’t start this line: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // An example name that isn't already a line, so the placeholder never reads as a clash.
  const example = NAME_EXAMPLES.find((n) => !taken.includes(n.toLowerCase())) ?? "Vendors";
  const spoken = (trimmed || example).toLowerCase();

  // Settled lines with the same scope first: those are the ones this line most likely continues.
  const seeds = [...settled].sort((a, b) => {
    const sa = sameScope(scopeOf(a), scope) ? 0 : 1;
    const sb = sameScope(scopeOf(b), scope) ? 0 : 1;
    return sa - sb || (settledAt(b) ?? "").localeCompare(settledAt(a) ?? "");
  });
  const seedLine = settled.find((l) => l.slug === seedFrom);

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
          <Button variant="primary" type="submit" form="conv-new-line" loading={busy} disabled={!trimmed || !!clash || !online}>
            Start {trimmed || "line"}
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
        <Field label="Name" error={shown} help={<>You’ll say {quoted(`patch me to ${spoken}`)}.</>}>
          <Input ref={nameRef} value={name} placeholder={example} maxLength={60} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label="Also answers to" optional help="Other names for it, separated by commas.">
          <Input value={aliases} placeholder={aliasExample(trimmed || example)} onChange={(e) => setAliases(e.target.value)} />
        </Field>
        <Field label="Persona" optional help="In your words. It goes into the prompt exactly as written. You can add it later.">
          <Textarea rows={4} value={persona} placeholder={PERSONA_EXAMPLE} onChange={(e) => setPersona(e.target.value)} />
        </Field>
        {(abilities === null || abilities.length > 0) && (
          <div className="conv-form__group">
            <p className="conv-form__label">
              Scope <span className="conv-form__optional">Optional</span>
            </p>
            <ScopePicker abilities={abilities} value={scope} onChange={setScope} idPrefix="conv-new-scope" />
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
      </form>
    </Dialog>
  );
}
