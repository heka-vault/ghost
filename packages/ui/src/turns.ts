/**
 * What the operator said, one message per thing said. The daemon keeps each stretch of
 * speech as its own turn, as it was heard, so a pause mid-sentence makes two ("Tell me a
 * little bit more about", "issue 415"). A run of the operator's spoken turns with nothing
 * between them (no reply, no note, no call) is one thing said, and that is how the model
 * is handed it too: a turn Ghost hadn't answered yet is reopened (`ghost_voice::turns`).
 * The words are kept as heard, joined with a space. Typed turns stay one message each.
 */
export function joinSpoken<T extends { role: string; text: string; via?: string | null }>(turns: readonly T[]): T[] {
  const out: T[] = [];
  for (const t of turns) {
    const last = out[out.length - 1];
    if (last && spoken(last) && spoken(t)) out[out.length - 1] = { ...last, text: `${last.text.trim()} ${t.text.trim()}` };
    else out.push(t);
  }
  return out;
}

/** The operator's words by voice: a stored turn doesn't say how it came, and most are. */
function spoken(t: { role: string; via?: string | null }): boolean {
  return t.role === "user" && t.via !== "text";
}
