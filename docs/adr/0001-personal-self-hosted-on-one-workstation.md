# 0001: Personal tool, self-hosted, entire stack on one always-on workstation

- **Status:** accepted
- **Date:** 2026-10-16

Ghost has one user and is not a product. It should cost nothing to run beyond LLM calls and depend on no hosted voice platform. The whole stack runs on one Linux desktop workstation (i5-11600K, 6C/12T with AVX-512, 32 GB, no GPU) that is on 24/7. It was the only available machine with headroom for local speech models.

## Considered options

- **LiveKit Cloud** (the original spec's media plane). Rejected: a vendor dependency for something we can run ourselves. Its free tier would have covered personal use.
- **A small always-on mini PC for the backend, workstation for speech.** Unnecessary once the workstation is known to never sleep.
- **An existing home server.** Rejected: 2 cores, no AVX2, and already busy with other workloads.

## Consequences

- The spec's multi-user pieces (`users`, `devices`, auth provider, row-level authorization) are out. Clients authenticate with per-device tokens kept in the operator's secrets store.
- If the workstation is down, Ghost is down. Accepted.
