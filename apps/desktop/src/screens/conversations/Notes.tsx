/**
 * Owner: conversations. Pinned notes: kept word for word, never summarized. "Remember
 * this" pins one now (an audited write, like saying it); editing or removing existing
 * notes is part of the draft and goes in with Save.
 */
import { Check, Pencil, Pin, Trash2, X } from "lucide-react";
import { useRef, useState } from "react";

import { Button, Card, EmptyState, IconButton, Input } from "../../ui";
import { NOTE_EXAMPLE } from "./examples";

export interface NotesProps {
  notes: string[];
  onChange: (notes: string[]) => void;
  onPin: (text: string) => Promise<boolean>;
  pinning: boolean;
  disabled: boolean;
  /** Notes that differ from what's saved, by text. */
  saved: string[];
}

export function Notes({ notes, onChange, onPin, pinning, disabled, saved }: NotesProps) {
  const [text, setText] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [edit, setEdit] = useState("");
  const listRef = useRef<HTMLDivElement>(null);

  const commit = (i: number) => {
    const t = edit.trim();
    const next = [...notes];
    if (t) next[i] = t;
    else next.splice(i, 1);
    onChange(next);
    setEditing(null);
  };

  const focusRowAction = (i: number) =>
    requestAnimationFrame(() => listRef.current?.querySelectorAll<HTMLElement>("[data-note-edit]")[i]?.focus());

  return (
    <div className="stack gap-3">
      <form
        className="conv-pin"
        onSubmit={async (e) => {
          e.preventDefault();
          const t = text.trim();
          if (!t || pinning || disabled) return;
          if (await onPin(t)) setText("");
        }}
      >
        <Input
          className="grow"
          aria-label="Remember this"
          placeholder={NOTE_EXAMPLE}
          leading={<Pin />}
          value={text}
          disabled={disabled}
          onChange={(e) => setText(e.target.value)}
        />
        <Button type="submit" variant="secondary" loading={pinning} disabled={disabled || !text.trim()}>
          Remember this
        </Button>
      </form>

      {notes.length === 0 ? (
        <Card padding="none">
          <EmptyState size="compact" title="No pinned notes" description="Pin what this line must never forget. Notes stay word for word, however long the line runs." />
        </Card>
      ) : (
        <Card list ref={listRef}>
          {notes.map((n, i) => (
            <div key={`${i}-${n}`} className="conv-note">
              {editing === i ? (
                <form
                  className="conv-note__edit"
                  onSubmit={(e) => {
                    e.preventDefault();
                    commit(i);
                    focusRowAction(i);
                  }}
                >
                  <Input
                    autoFocus
                    size="sm"
                    className="grow"
                    aria-label={`Edit note ${i + 1}`}
                    placeholder="Leave empty to remove it"
                    value={edit}
                    onChange={(e) => setEdit(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        e.preventDefault();
                        e.stopPropagation();
                        setEditing(null);
                        focusRowAction(i);
                      }
                    }}
                  />
                  <IconButton type="submit" size="sm" tooltipSide="bottom" label="Keep this edit" icon={<Check />} />
                  <IconButton
                    size="sm"
                    tooltipSide="bottom"
                    label="Cancel edit"
                    icon={<X />}
                    onClick={() => {
                      setEditing(null);
                      focusRowAction(i);
                    }}
                  />
                </form>
              ) : (
                <>
                  <Pin size={14} aria-hidden className="conv-note__pin" />
                  <p className="conv-note__text" data-user-content>
                    {n}
                  </p>
                  {!saved.includes(n) && <span className="conv-note__new t-caption">Not saved</span>}
                  <span className="conv-note__actions">
                    <IconButton
                      data-note-edit
                      size="sm"
                      label="Edit note"
                      icon={<Pencil />}
                      disabled={disabled}
                      onClick={() => {
                        setEdit(n);
                        setEditing(i);
                      }}
                    />
                    <IconButton
                      size="sm"
                      label="Remove note"
                      icon={<Trash2 />}
                      disabled={disabled}
                      onClick={() => {
                        onChange(notes.filter((_, j) => j !== i));
                        requestAnimationFrame(() => listRef.current?.querySelectorAll<HTMLElement>("[data-note-edit]")[Math.min(i, notes.length - 2)]?.focus());
                      }}
                    />
                  </span>
                </>
              )}
            </div>
          ))}
        </Card>
      )}
    </div>
  );
}
