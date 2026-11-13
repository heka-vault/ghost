#!/usr/bin/env bash
# What building and running Ghost from a checkout needs, checked: the system packages the
# workspace links, the toolchains, and the desktop session's services. Says what is missing
# and the command that installs it (Ubuntu/Debian, Fedora, Arch); never installs anything
# and never runs sudo. `make prereqs`, and the first thing `make setup` does.
#
#   scripts/prereqs.sh          check; exit 1 when something the build needs is missing
#   scripts/prereqs.sh --print  the package list for this distribution, then exit
#
# Build needs fail the check. Run-time needs (PipeWire, a Secret Service keyring, a display)
# are warnings: the build works without them, and README.md says what Ghost does without each.
set -uo pipefail

NODE_MIN=22
ROOT=$(cd "$(dirname "$0")/.." && pwd)

if [[ -t 1 ]]; then red=$'\e[31m' yellow=$'\e[33m' green=$'\e[32m' off=$'\e[0m'; else red='' yellow='' green='' off=''; fi
missing_pkgs=() problems=() warnings=()

distro=unknown
if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  case " ${ID:-} ${ID_LIKE:-} " in
    *" debian "* | *" ubuntu "*) distro=debian ;;
    *" fedora "* | *" rhel "*) distro=fedora ;;
    *" arch "*) distro=arch ;;
  esac
fi

# One row per need: what is checked | Debian/Ubuntu package | Fedora package | Arch package.
# A check is `cmd:<name>`, `pc:<pkg-config module>` or `file:<path glob>`.
NEEDS=(
  "cmd:cc|build-essential|gcc|base-devel"
  "cmd:c++|build-essential|gcc-c++|base-devel"
  "cmd:make|build-essential|make|base-devel"
  "cmd:cmake|cmake|cmake|cmake"
  "cmd:pkg-config|pkg-config|pkgconf-pkg-config|pkgconf"
  "cmd:git|git|git|git"
  "cmd:curl|curl|curl|curl"
  "cmd:xz|xz-utils|xz|xz"
  "cmd:clang|clang|clang|clang"
  "cmd:ld.lld|lld|lld|lld"
  "file:/usr/lib*/**/libclang*.so*|libclang-dev|clang-devel|clang"
  "pc:openssl|libssl-dev|openssl-devel|openssl"
  "pc:webkit2gtk-4.1|libwebkit2gtk-4.1-dev|webkit2gtk4.1-devel|webkit2gtk-4.1"
  "pc:gtk+-3.0|libgtk-3-dev|gtk3-devel|gtk3"
  "pc:ayatana-appindicator3-0.1|libayatana-appindicator3-dev|libayatana-appindicator-gtk3-devel|libayatana-appindicator"
  "pc:librsvg-2.0|librsvg2-dev|librsvg2-devel|librsvg"
  "file:/usr/include/xdo.h|libxdo-dev|libxdo-devel|xdotool"
  "pc:libpipewire-0.3|libpipewire-0.3-dev|pipewire-devel|libpipewire"
  "pc:libspa-0.2|libspa-0.2-dev|pipewire-devel|libpipewire"
  "pc:libpulse|libpulse-dev|pulseaudio-libs-devel|libpulse"
  "pc:alsa|libasound2-dev|alsa-lib-devel|alsa-lib"
  "file:/usr/include/espeak-ng/speak_lib.h|libespeak-ng-dev|espeak-ng-devel|espeak-ng"
  "pc:opus|libopus-dev|opus-devel|opus"
)

col() { case $distro in debian) echo "$2" ;; fedora) echo "$3" ;; arch) echo "$4" ;; *) echo "$2" ;; esac; }

all_pkgs() {
  local row seen=" " p
  for row in "${NEEDS[@]}"; do
    IFS='|' read -r _ deb fed arch <<<"$row"
    p=$(col x "$deb" "$fed" "$arch")
    [[ $seen == *" $p "* ]] || { seen+="$p "; printf '%s ' "$p"; }
  done
}

install_cmd() {
  case $distro in
    fedora) echo "sudo dnf install $*" ;;
    arch) echo "sudo pacman -S --needed $*" ;;
    *) echo "sudo apt install $*" ;;
  esac
}

if [[ ${1:-} == --print ]]; then
  install_cmd "$(all_pkgs)"
  exit 0
fi

have() { command -v "$1" >/dev/null 2>&1; }

check() {
  local what=$1
  case $what in
    cmd:*) have "${what#cmd:}" ;;
    pc:*) have pkg-config && pkg-config --exists "${what#pc:}" ;;
    file:*)
      local f
      shopt -s globstar nullglob
      for f in ${what#file:}; do
        [[ -e $f ]] && { shopt -u globstar nullglob; return 0; }
      done
      shopt -u globstar nullglob
      return 1
      ;;
  esac
}

for row in "${NEEDS[@]}"; do
  IFS='|' read -r what deb fed arch <<<"$row"
  if ! check "$what"; then
    p=$(col x "$deb" "$fed" "$arch")
    [[ " ${missing_pkgs[*]} " == *" $p "* ]] || missing_pkgs+=("$p")
  fi
done

# The toolchains, each from its own installer: rustup reads rust-toolchain.toml and fetches
# the pinned Rust on first use; Node 22 or later with corepack for the pinned pnpm; uv for
# the Python tools.
if ! have rustup && ! have cargo; then
  problems+=("Rust: install rustup (https://rustup.rs): curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   (the version is pinned in rust-toolchain.toml and fetched on first build)")
elif ! have rustup; then
  warnings+=("cargo is there but rustup is not: the build wants Rust $(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml") (rust-toolchain.toml); rustup fetches it for you")
fi
if ! have node; then
  problems+=("Node.js $NODE_MIN or later: from https://nodejs.org, your distribution, or a version manager (fnm, nvm, mise)")
else
  v=$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)
  ((v >= NODE_MIN)) || problems+=("Node.js $NODE_MIN or later (this is $(node -v))")
fi
if ! have pnpm; then
  problems+=("pnpm: corepack enable   (Node's own; the version is pinned in package.json; add --install-directory ~/.local/bin when Node's folder is not writable)")
fi
if ! have uv; then
  problems+=("uv, for the Python tools and the checks: curl -LsSf https://astral.sh/uv/install.sh | sh")
fi

# The checks before a pull request (make ci): notes, since building and running need neither.
for tool in nextest deny; do
  if have cargo && ! cargo "$tool" --version >/dev/null 2>&1; then
    warnings+=("cargo-$tool, for make ci: cargo install --locked cargo-$tool")
  fi
done

# Running Ghost (warnings): sound, the keyring, a display.
if ! have pactl; then
  warnings+=("No pactl: it lists your microphones and \`make echo-cancel\` uses it; $(install_cmd "$(col x pulseaudio-utils pulseaudio-utils libpulse)")")
elif ! pactl info >/dev/null 2>&1; then
  warnings+=("No PipeWire (or PulseAudio) server answers: Ghost needs PipeWire with pipewire-pulse to hear and speak. Typing to it works without.")
elif ! pactl info 2>/dev/null | grep -q 'PipeWire'; then
  warnings+=("The sound server is not PipeWire: Ghost's audio is PipeWire's own API, so it will not hear or speak. Typing to it works.")
fi
if have busctl && ! busctl --user status org.freedesktop.secrets >/dev/null 2>&1 \
  && ! busctl --user list 2>/dev/null | grep -q 'org.freedesktop.secrets'; then
  warnings+=("No Secret Service keyring on your session bus: Ghost keeps its passwords in a vault whose key is in that keyring, so without one it cannot hold a model key (a local Ollama or LM Studio needs none). GNOME Keyring, KDE Wallet with its Secret Service, or KeePassXC's provide it. README.md, \"Passwords\".")
fi
if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
  warnings+=("No display: the desktop app needs a graphical session; the daemon alone runs without one (make daemon).")
fi

echo "Ghost's prerequisites on ${PRETTY_NAME:-this system} ($distro):"
status=0
if ((${#missing_pkgs[@]})); then
  status=1
  echo "${red}missing system packages:${off} ${missing_pkgs[*]}"
  echo "  $(install_cmd "${missing_pkgs[*]}")"
  [[ $distro == unknown ]] && echo "  (names are Ubuntu's; scripts/prereqs.sh lists Fedora's and Arch's too)"
fi
for p in "${problems[@]}"; do status=1; echo "${red}missing:${off} $p"; done
for w in "${warnings[@]}"; do echo "${yellow}note:${off} $w"; done
if ((status == 0)); then echo "${green}everything the build needs is here${off}"; fi
exit $status
