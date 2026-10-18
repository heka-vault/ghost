/**
 * Owner: conversations. Start a new line: a name (what you say after "patch me to"),
 * other names it answers to, and an optional persona in the operator's own words.
 */
import type { ConversationDetail } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import { Button, Dialog, Field, Input, Textarea, errorText, quoted, useLive } from "../../ui";
import { splitAliases } from "./draft";
import { aliasExample, PERSONA_EXAMPLE } from "./examples";

const NAME_EXAMPLES = ["Vendors", "Legal", "Hiring", "Release train", "Security", "Support"];

export interface NewLineDialogProps {
  open: boolean;
  onClose: () => void;
  /** Names already taken, lowercased. */
  taken: string[];
  onCreated: (d: ConversationDetail) => void;
}

export function NewLineDialog({ open, onClose, taken, onCreated }: NewLineDialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [name, setName] = useState("");
  const [aliases, setAliases] = useState("");
  const [persona, setPersona] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setName("");
      setAliases("");
      setPersona("");
      setError(null);
      // showModal() focuses the close button; the name is where the operator starts.
      const id = requestAnimationFrame(() => nameRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [open]);

  const trimmed = name.trim();
  const clash = trimmed && taken.includes(trimmed.toLowerCase()) ? `There’s already a line called ${trimmed}. Pick another name.` : null;
  const shown = clash ?? error;

  const submit = async () => {
    if (!trimmed || clash || busy) return;
    setBusy(true);
    setError(null);
    try {
      const d = await client.createConversation({ name: trimmed, aliases: splitAliases(aliases), voice: null, speed: null, persona });
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
      </form>
    </Dialog>
  );
}
