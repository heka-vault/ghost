/**
 * Owner: settings. Ghost's wake phrases (GHOST_WAKE_PHRASES): one field, shown on Sound and
 * on Ghost's own agent page. One setting, two ways in; each page saves it with its own Save.
 * The caller wraps it in its card row.
 */
import type { ReactNode } from "react";

import { Field } from "../../ui";
import { ChipsInput } from "./parts";
import "./settings.css";

export const WAKE_KEY = "GHOST_WAKE_PHRASES";

/** The help line under the field: what the list does, or what empty means. */
export const wakeHelp = (count: number) => (count ? "Any of these starts a session. Two or three words work best." : "Empty means “hey ghost”.");

const check = (entry: string) => (entry.split(/\s+/).length > 5 ? "Keep a wake phrase to five words or fewer." : null);

export function WakePhrasesField({
  id,
  phrases,
  onChange,
  help,
  error,
  onEntryError,
  disabled,
}: {
  id?: string;
  phrases: string[];
  onChange: (phrases: string[]) => void;
  help?: ReactNode;
  error?: ReactNode;
  onEntryError?: (message: string | null) => void;
  disabled?: boolean;
}) {
  return (
    <Field id={id} label="Wake phrases" detail={WAKE_KEY} help={help} error={error}>
      <ChipsInput kind="phrase" noun="phrase" items={phrases} placeholder="hey ghost" check={check} onEntryError={onEntryError} onChange={onChange} disabled={disabled} />
    </Field>
  );
}
