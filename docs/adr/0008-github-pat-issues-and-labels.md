# 0008: GitHub access through a fine-grained PAT; Issues and labels only

- **Status:** accepted
- **Date:** 2026-10-16
- **Supersedes:** original spec section 7.1 (repository-scoped GitHub App)

A GitHub App is worth it for multiple users or installations. For one person, a fine-grained personal access token limited to the chosen repositories (Metadata read, Issues read and write) does the job with no app registration, private key, or installation-token minting. It is kept in the operator's secrets store and given to Ghost as `GITHUB_TOKEN`. Workflow state is labels on GitHub Issues; Projects v2 boards are not used.

## Consequences

- Writes appear as the operator's own GitHub account. Ghost's comments carry a hidden marker so they can be told apart and deduplicated.
- The repository allowlist is enforced by Ghost's config as well as by the token's scope.
