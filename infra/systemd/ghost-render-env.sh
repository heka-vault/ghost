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
KEYS=(ANTHROPIC_BASE_URL ANTHROPIC_AUTH_TOKEN CLIPROXY_BASE_URL GITHUB_TOKEN GHOST_REPOS HF_TOKEN TYPESAFE_API_KEY OPENROUTER_API_KEY)

if [[ ! -r "$SECRETS_ENV" ]]; then
  # No secrets store on this machine: the desktop app writes $OUT instead. Not an error.
  echo "ghost-render-env: no $SECRETS_ENV; using $OUT as written by the app" >&2
  exit 0
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

# Per-owner GitHub tokens (ADR 0019): GHOST_GITHUB_TOKENS="owner=ENV_NAME,..." in
# settings.env names the variables; fetch each of those from the store too.
if [[ -r "$OUT_DIR/settings.env" ]]; then
  map="$(sed -n 's/^GHOST_GITHUB_TOKENS=//p' "$OUT_DIR/settings.env" | tr -d '"' | tail -1)"
  IFS=',' read -ra pairs <<<"$map"
  for pair in "${pairs[@]}"; do
    name="${pair#*=}"; name="${name// /}"
    [[ -n "$name" && "$name" != "$pair" ]] && KEYS+=("$name")
  done
fi

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
