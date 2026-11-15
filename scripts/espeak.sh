#!/usr/bin/env bash
# espeak-ng's data for the bundle (#168, ADR 0075). The AppImage carries the build machine's
# libespeak-ng.so.1 (linuxdeploy copies it), and that library reads the data folder its own
# package put beside it; a machine of another family keeps its data elsewhere, made for another
# version, or has none. So the bundle carries the data that goes with the library it carries:
# the same build machine's espeak-ng-data package, checked to be the library's own version,
# copied with its copyright file and a stamp naming that version, which the daemon checks
# against the library it loads (ghost_infer::espeak::STAMP) before using it.
#
#   scripts/espeak.sh stage    copy it to apps/desktop/src-tauri/resources/espeak-ng-data/
#
# The source is the Debian or Ubuntu packages `libespeak-ng1` and `espeak-ng-data` the build
# installs (libespeak-ng-dev pulls both), never a download: they must be the library's own.
# GPL-3.0-or-later, as the library the bundle already carries is; the corresponding source is
# that package's source (`apt-get source espeak-ng=<version>`), named in SOURCE beside it.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEST="$ROOT/apps/desktop/src-tauri/resources/espeak-ng-data"
STAMP=GHOST-ESPEAK-NG-VERSION

case "${1:-}" in
  stage) ;;
  *) echo "usage: $0 stage" >&2; exit 2 ;;
esac

command -v dpkg-query >/dev/null || {
  echo "espeak: the bundle is built on Debian or Ubuntu (dpkg-query is not here); see docs/releasing.md" >&2
  exit 1
}
lib_version=$(dpkg-query -W -f='${Version}' libespeak-ng1 2>/dev/null || true)
data_version=$(dpkg-query -W -f='${Version}' espeak-ng-data 2>/dev/null || true)
[ -n "$lib_version" ] || { echo "espeak: libespeak-ng1 is not installed (apt-get install libespeak-ng-dev)" >&2; exit 1; }
[ -n "$data_version" ] || { echo "espeak: espeak-ng-data is not installed (apt-get install espeak-ng-data)" >&2; exit 1; }
if [ "$lib_version" != "$data_version" ]; then
  echo "espeak: libespeak-ng1 is $lib_version but espeak-ng-data is $data_version; they must be the same package version" >&2
  exit 1
fi
arch=$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo "$(uname -m)-linux-gnu")
SRC="/usr/lib/$arch/espeak-ng-data"
[ -f "$SRC/phontab" ] || { echo "espeak: $SRC has no phontab" >&2; exit 1; }
# The upstream release, as espeak_Info names it: 1.51 from 1.51+dfsg-12build1.
release=${lib_version#*:}
release=${release%%[+~-]*}

# Ghost speaks English only (ghost_voice::tts::LANG, held by a test there): the phoneme tables,
# the language and voice lists, and English's dictionary, 2.6 MB of the folder's 25 MB. A
# language Ghost learns to speak adds its `<code>_dict` here.
DICTS="en_dict"
rm -rf "$DEST"
mkdir -p "$DEST"
for f in "$SRC"/*; do
  name=$(basename "$f")
  case "$name" in
    *_dict) case " $DICTS " in *" $name "*) ;; *) continue ;; esac ;;
  esac
  cp -a "$f" "$DEST/"
done
cp /usr/share/doc/espeak-ng-data/copyright "$DEST/COPYRIGHT"
{
  echo "$release"
  echo "espeak-ng-data $data_version, from the build machine's packages, for libespeak-ng1 $lib_version"
} > "$DEST/$STAMP"
cat > "$DEST/SOURCE" <<EOF
espeak-ng's data, GPL-3.0-or-later (COPYRIGHT), as Debian/Ubuntu package it:
espeak-ng-data $data_version, for the libespeak-ng.so.1 ($lib_version) this bundle carries.
The corresponding source is that package's source: apt-get source espeak-ng=$lib_version
(https://packages.ubuntu.com/source/espeak-ng, https://github.com/espeak-ng/espeak-ng).
EOF
echo "espeak: $DEST ($release, $(du -sh "$DEST" | cut -f1))" >&2
echo "$DEST"
