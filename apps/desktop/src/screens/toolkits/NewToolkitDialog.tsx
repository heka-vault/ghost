/**
 * Owner: toolkits. Start a toolkit: its name (the key in the policy file, fixed once saved)
 * and a line about it. It opens in the designer unsaved; Save writes it to the file.
 */
import { useEffect, useRef, useState } from "react";

import { Button, Dialog, Field, Input } from "../../ui";
import { NAME_RE } from "./model";

const EXAMPLES = ["finance", "code", "ops", "design", "support", "hiring"];

export interface NewToolkitDialogProps {
  open: boolean;
  onClose: () => void;
  /** Names already in the file or waiting to be saved. */
  taken: string[];
  onCreate: (name: string, description: string) => void;
}

export function NewToolkitDialog({ open, onClose, taken, onCreate }: NewToolkitDialogProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      const id = requestAnimationFrame(() => nameRef.current?.focus());
      return () => cancelAnimationFrame(id);
    }
  }, [open]);

  const trimmed = name.trim();
  const example = EXAMPLES.find((n) => !taken.includes(n)) ?? "finance";
  const error = !trimmed ? null : taken.includes(trimmed) ? `There’s already a toolkit called ${trimmed}. Pick another name.` : !NAME_RE.test(trimmed) ? "Lowercase letters, digits, _ and -, starting with a letter or digit, up to 40 characters." : null;
  const ok = !!trimmed && !error;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title="New toolkit"
      description="What a department may reach: servers, their tools and items, and skills. It only narrows your policy file."
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="tk-new" disabled={!ok}>
            Design {trimmed || "it"}
          </Button>
        </>
      }
    >
      <form
        id="tk-new"
        className="tk-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) onCreate(trimmed, description.trim());
        }}
      >
        <Field label="Name" error={error} help={<>Its key in the policy file: <span className="t-mono">[toolkit.{trimmed || example}]</span>. Fixed once saved.</>}>
          <Input ref={nameRef} mono value={name} placeholder={example} maxLength={40} autoComplete="off" spellCheck={false} onChange={(e) => setName(e.target.value.toLowerCase())} />
        </Field>
        <Field label="Description" optional help="One line in your words. You can change it any time.">
          <Input value={description} placeholder="The books: billing issues, invoices and the month close." maxLength={200} onChange={(e) => setDescription(e.target.value)} />
        </Field>
      </form>
    </Dialog>
  );
}
