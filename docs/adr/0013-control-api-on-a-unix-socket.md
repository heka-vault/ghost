# 0013: A local control API on a Unix socket; TypeScript apps are generated clients

- **Status:** accepted
- **Date:** 2026-10-17
- **Amends:** ADR 0007 (confirmation evidence)

Ghost ships as a Debian package with a desktop app, and later a phone app, both in
TypeScript. The audio stack stays Python (ADR 0003). The apps need a typed contract to the
daemon, not bindings.

`ghostd` serves a FastAPI app on `$XDG_RUNTIME_DIR/ghost/api.sock`, mode 0600. No TCP
port. It exposes status and a WebSocket of events, wake and sleep, a typed turn into the
session, the pending confirm-level change with confirm and cancel, undo, the audit log,
abilities with their resolved autonomy, the policy file as a validated view, write-only
secrets, audio devices, and first-run setup. The OpenAPI document is generated from the
pydantic models and committed at `packages/api/openapi.json`; `packages/client-ts` is
generated from it and a test fails when either is stale.

The phone does not use this API. It joins the pipeline as a Pipecat WebRTC transport
(ADR 0005), so the gate and the policy apply unchanged and the control API never leaves
the machine.

## Consequences

- A click on Confirm in the operator's own desktop session is confirmation evidence,
  alongside the transcript. The gate records which path confirmed.
- The API carries session turns, never passive audio or passive transcripts. Secrets go
  in and never come out; the UI only learns whether one is set.
- The model has no route to this API. It is a surface for the operator's own tools.
- `ghostd wake`, `say`, `sleep` and `status` become clients of the socket.
