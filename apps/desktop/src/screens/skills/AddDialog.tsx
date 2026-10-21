/**
 * Owner: skills. Add a skill by naming its folder: the way without a file manager, and the
 * only way in a browser, which never gives a dropped folder's path. The daemon's refusal
 * shows inline; when it says the name is already installed, the primary action turns into
 * Replace, which adds again with `replace`.
 */
import type { SkillInfo } from "@ghost/client-ts";
import { useEffect, useRef, useState } from "react";

import { Button, Dialog, Field, Input, Notice, errorText } from "../../ui";
import { GHOST_SKILLS_DIR, saysInstalled } from "./model";

export interface AddDialogProps {
  open: boolean;
  busy: boolean;
  onClose: () => void;
  /** Copies the folder; throws the daemon's refusal. */
  onAdd: (path: string, replace?: boolean) => Promise<SkillInfo>;
}

const FORM_ID = "skl-add-form";

export function AddDialog({ open, busy, onClose, onAdd }: AddDialogProps) {
  const [path, setPath] = useState("");
  /** The daemon's refusal for the path as typed. */
  const [refusal, setRefusal] = useState<string | null>(null);
  const [empty, setEmpty] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setPath("");
    setRefusal(null);
    setEmpty(false);
  }, [open]);

  const submit = async (replace = false) => {
    const p = path.trim();
    if (!p) {
      setEmpty(true);
      input.current?.focus();
      return;
    }
    try {
      await onAdd(p, replace);
      onClose();
    } catch (e) {
      setRefusal(errorText(e));
    }
  };
  const installed = !!refusal && saysInstalled(refusal);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      initialFocus={input}
      title="Add a skill from a folder"
      description={
        <>
          The folder that holds <span className="t-mono">SKILL.md</span>. Ghost copies it into <span className="t-mono">{GHOST_SKILLS_DIR}</span>.
        </>
      }
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          {installed ? (
            <Button variant="primary" loading={busy} onClick={() => void submit(true)}>
              Replace
            </Button>
          ) : (
            <Button variant="primary" type="submit" form={FORM_ID} loading={busy}>
              Add
            </Button>
          )}
        </>
      }
    >
      <form
        id={FORM_ID}
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Folder" help="A full path; ~ is your home folder." error={empty ? "Type the folder’s path, like ~/Downloads/invoicing." : undefined}>
          <Input
            ref={input}
            mono
            value={path}
            placeholder="~/Downloads/invoicing"
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
            onChange={(e) => {
              setPath(e.target.value);
              setRefusal(null);
              setEmpty(false);
            }}
          />
        </Field>
      </form>
      {refusal && (
        <Notice tone="danger" title={installed ? "Already installed" : "Couldn’t add that folder"} className="skl-add__notice">
          <p className="skl-notice__p" data-user-content>
            {refusal}
          </p>
          <p className="skl-notice__p">{installed ? "Replace it to use this folder’s copy instead; the one Ghost has is deleted." : "Point at the folder that holds SKILL.md."}</p>
        </Notice>
      )}
    </Dialog>
  );
}
