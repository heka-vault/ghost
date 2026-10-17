#!/usr/bin/env bash
# Render Ghost's runtime environment file from the operator's secrets store.
#
# Reads the store's URL, project, and read token from $GHOST_SECRETS_ENV
# (default ~/.config/heka-vault/ghost.env, mode 600) and writes every secret in
# the project to ~/.config/ghost/env (mode 600) as KEY=value lines, plus the
# non-secret settings in ~/.config/ghost/settings.env if present.
#
# The store is any HTTP service that answers
#   GET $URL/api/v1/projects/$PROJECT/secrets/$KEY?env=dev&format=raw
# with a bearer token. Values never reach stdout or the journal.
set -euo pipefail

SECRETS_ENV="${GHOST_SECRETS_ENV:-$HOME/.config/heka-vault/ghost.env}"
OUT_DIR="$HOME/.config/ghost"
OUT="$OUT_DIR/env"
KEYS=(ANTHROPIC_BASE_URL ANTHROPIC_AUTH_TOKEN CLIPROXY_BASE_URL GITHUB_TOKEN GHOST_REPOS HF_TOKEN)

if [[ ! -r "$SECRETS_ENV" ]]; then
  echo "ghost-render-env: $SECRETS_ENV missing; keeping existing $OUT" >&2
  [[ -r "$OUT" ]] && exit 0
  exit 1
fi

set -a
# shellcheck disable=SC1090
. "$SECRETS_ENV"
set +a
: "${HEKA_VAULT_URL:?}" "${HEKA_VAULT_PROJECT:?}" "${HEKA_VAULT_TOKEN:?}"

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"
tmp="$(mktemp "$OUT_DIR/.env.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
chmod 600 "$tmp"

for key in "${KEYS[@]}"; do
  if value="$(curl -fsS --max-time 10 -H "Authorization: Bearer $HEKA_VAULT_TOKEN" \
      "$HEKA_VAULT_URL/api/v1/projects/$HEKA_VAULT_PROJECT/secrets/$key?env=dev&format=raw" 2>/dev/null)"; then
    printf '%s=%s\n' "$key" "$value" >>"$tmp"
  fi
done

# The LLM endpoint falls back to the proxy URL when no explicit base URL is stored.
if ! grep -q '^ANTHROPIC_BASE_URL=' "$tmp" && grep -q '^CLIPROXY_BASE_URL=' "$tmp"; then
  sed -n 's/^CLIPROXY_BASE_URL=/ANTHROPIC_BASE_URL=/p' "$tmp" >>"$tmp"
fi

# Non-secret overrides (GHOST_LLM_MODEL, GHOST_DUCK_APPS, ...), operator-edited.
[[ -r "$OUT_DIR/settings.env" ]] && cat "$OUT_DIR/settings.env" >>"$tmp"

mv "$tmp" "$OUT"
trap - EXIT
echo "ghost-render-env: wrote $(grep -c . "$OUT") keys to $OUT" >&2
