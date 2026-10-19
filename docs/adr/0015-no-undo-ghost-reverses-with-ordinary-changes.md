# 0015: No undo in the system; Ghost reverses a change with an ordinary change

- **Status:** accepted
- **Date:** 2026-10-19
- **Amends:** ADR 0011 (removes the undo path), ADR 0012 (interceptors no longer undo), ADR 0013 (no `/undo`), ADR 0014 (conversation writes are not undoable)

ADR 0011 added an undo path: every change type could invert itself against its snapshot, and "undo that" reversed the session's last change within ten minutes, through a quick intent, an LLM tool, the control API and buttons in every app. The operator removed it. When something should be changed back, the operator says so and Ghost works out the opposite change itself (reopen the issue, put the labels back, restore the note) and proposes it through `change`, where the gate and the operator's policy treat it like any other write.

## Consequences

- One write path. Reversing a change is a change, checked against the policy for its own operation, so reopening asks if reopening is set to ask. There is no separate, unpoliced undo route to secure or test.
- Change types no longer implement `invert`, and the change types that existed only to be produced by undo are gone. A reversal is limited to what an ability's operations can do: Ghost can reopen and relabel, but it cannot delete a comment it added unless an ability offers that operation.
- "Undo that" reaches the model as an ordinary turn. Ghost knows what it changed from the session transcript and the outcome it spoke; the audit log keeps every snapshot for the operator.
- ADR 0011's trade-off stands without its safety net: writes the operator lets happen without asking are reversed by asking Ghost, not by a button. The policy file is where that risk is tuned.
- Audit rows written before this decision may carry the status `undone` or an `undo:` operation. They are history and stay readable.
