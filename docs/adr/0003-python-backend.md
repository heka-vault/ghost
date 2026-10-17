# 0003: Python for everything except the mobile app

- **Status:** accepted
- **Date:** 2026-10-16
- **Supersedes:** original spec's NestJS/TypeScript API and Node agent

Pipecat, the local speech models (Parakeet via ONNX, Kokoro), and the wake-word tooling are all Python. Splitting the backend into a TypeScript API and a Python agent would mean two languages and an internal network contract for no benefit at one user. The daemon, voice pipeline, skills, and domain are one `uv` workspace on Python 3.12. The mobile app, when it comes, stays Expo/TypeScript.

## Consequences

- The domain write gate stays a separate package with its own boundary (ADR 0007), even though it runs in the same process as the agent.
