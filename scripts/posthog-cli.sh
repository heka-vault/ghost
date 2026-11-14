#!/usr/bin/env bash
# posthog-cli 0.20.0, which uploads the release's page source maps to PostHog's error
# tracking (ADR 0067, docs/releasing.md): fetched from PostHog's GitHub release, pinned by
# SHA-256 (the digest GitHub reports for the asset, and the one PostHog's own installer
# carries), into <dir>. Only the release workflow runs it; `make dev` and `make bundle` never
# do. Nothing here reads a key or talks to PostHog.
#
#   scripts/posthog-cli.sh <dir>    fetch and verify; the binary is <dir>/posthog-cli
#   scripts/posthog-cli.sh --help   this
#
# The release build writes the chunk ids itself (tools/report_chunks.py): `posthog-cli
# sourcemap inject` needs the personal key and talks to PostHog, and a dry run must do
# neither, so only `upload` is the CLI's, in the tag's own step.
set -euo pipefail

VERSION=0.20.0
ARCHIVE=posthog-cli-x86_64-unknown-linux-gnu.tar.gz
URL="https://github.com/PostHog/posthog/releases/download/posthog-cli/v$VERSION/$ARCHIVE"
SHA256=5417354c2faab32e377be178382d28e2e327ecce85858e3e05e905657986c9b9

case "${1:-}" in
  -h | --help | help | "") sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; [ -n "${1:-}" ]; exit $? ;;
esac
DIR=$1

tmp=$(mktemp -d "${TMPDIR:-/tmp}/posthog-cli.XXXXXX")
# shellcheck disable=SC2064
trap "rm -rf '$tmp'" EXIT
echo "posthog-cli: fetching $URL" >&2
curl -fsSL --retry 3 -o "$tmp/$ARCHIVE" "$URL"
got=$(sha256sum "$tmp/$ARCHIVE" | cut -d' ' -f1)
if [ "$got" != "$SHA256" ]; then
  echo "posthog-cli: $ARCHIVE does not match the pinned SHA-256" >&2
  echo "  expected $SHA256" >&2
  echo "  got      $got" >&2
  echo "  the archive is not used; check the release and the pin in $0" >&2
  exit 1
fi
tar -xzf "$tmp/$ARCHIVE" -C "$tmp"
mkdir -p "$DIR"
cp -R "$tmp/posthog-cli-x86_64-unknown-linux-gnu/." "$DIR/"
chmod 755 "$DIR/posthog-cli"
echo "posthog-cli: $DIR/posthog-cli ($VERSION, verified)" >&2
