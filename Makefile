# Ghost's entry points. `make` alone lists them, grouped by what you are doing.
#
# Working on Ghost:
#   make setup     once per machine: dependencies, Ghost's folder, the echo-cancelled mic
#   make dev       the daemon and the desktop app from this checkout, both logging to this
#                  terminal; the UI reloads on save, and Ctrl+C stops both
#   make reload    in another terminal after a Python change: restarts the daemon, not the app
#   make say TEXT="what needs me?"   types a turn to it; or say "Hey Ghost"
#
# Ghost ships as one desktop bundle with the daemon inside (ADR 0038): `make bundle` makes
# the .deb and the AppImage, and the installed app runs ghostd while you are logged in. The
# ghostd user service is the headless mode, running a fixed build of a commit that `make
# deploy` made. Only one Ghost may have the microphone, so `make dev` stops the service and
# `make service-on` starts it again; the app adopts a daemon already on the socket.
#
# Passwords never live in this repo or in a plain file: they are in Ghost's own vault, its key
# in your keyring, and the daemon opens it itself (ADR 0036; `make passwords`). The environment
# the targets load is $(ENV_FILE), non-secret settings only: settings.env, or a ./.env you made
# from .env.example.

SHELL := /bin/bash
.DEFAULT_GOAL := help

ENV_FILE ?= $(if $(wildcard .env),.env,$(HOME)/.config/ghost/settings.env)
SETTINGS := $(HOME)/.config/ghost/settings.env
POLICY := $(HOME)/.config/ghost/policy.toml
UV ?= uv
export ENV_FILE UV

# This checkout's daemon and no other (Python in .venv, or the Rust one in target/debug):
# the service's ghostd runs from a build elsewhere.
DEV_GHOSTD := $(CURDIR)/(.venv/bin|target/debug)/ghostd run$$

define load_env
set -a; [[ -r "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a;
endef

define no_dev_daemon
if pgrep -f '$(DEV_GHOSTD)' >/dev/null; then \
  echo "This checkout's ghostd is running; stop it first (Ctrl+C in make dev, or make stop)."; exit 1; fi;
endef

.PHONY: help setup sync env passwords passwords-import echo-cancel echo-cancel-off \
        dev dev-rust daemon desktop desktop-web reload start stop restart \
        wake say status check devices latency restore-volumes \
        lint fmt test typecheck ci api-schema version release \
        deploy deploy-rollback deploy-status service-on service-off service-logs service-install \
        perf perf-micro perf-llm record wake-eval record-takes wake-takes-eval \
        android android-apk android-install mobile-web \
        bundle sidecars build-image deb deb-test

help: ## list the targets
	@awk 'BEGIN {FS = ":.*## "} /^##@ / {printf "\n%s\n", substr($$0, 5)} /^[a-z][a-z-]*:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

##@ Set up, once per machine (safe to repeat)

setup: ## sync, pnpm install, env, echo-cancel and check, then says what is left to do by hand
	@if dpkg -l 'heka-ghost*' 2>/dev/null | grep -q '^ii'; then \
	  echo "The heka-ghost packages are installed. Remove them so only this checkout runs:"; \
	  echo "  sudo apt remove heka-ghost heka-ghost-desktop heka-ghostd"; echo; fi
	$(UV) sync --all-packages
	pnpm install
	@$(MAKE) --no-print-directory env
	@systemctl --user is-active --quiet ghost-echo-cancel || infra/pipewire/install.sh
	@$(load_env) $(UV) run ghostd check
	@echo; echo "Ready: make dev starts the daemon and the desktop app from this checkout."

sync: ## install or refresh the Python workspace (make dev does this on every start)
	$(UV) sync --all-packages

env: ## seed settings.env, policy.toml and Ghost's folder on first run (passwords: make passwords)
	@if [[ ! -e "$(SETTINGS)" ]]; then \
	  mkdir -p "$(dir $(SETTINGS))"; \
	  printf 'GHOST_INPUT_DEVICE=ghost.mic\n' > "$(SETTINGS)"; \
	  echo "seeded $(SETTINGS) (ghost.mic); edit it for non-secret settings"; \
	fi
	@if [[ ! -e "$(POLICY)" ]]; then \
	  mkdir -p "$(dir $(POLICY))"; cp infra/policy.example.toml "$(POLICY)"; \
	  echo "seeded $(POLICY); edit it to block what Ghost may not do, or ask first"; \
	fi
	@for f in apps.json AGENTS.md; do \
	  if [[ ! -e "$(dir $(POLICY))$$f" ]]; then \
	    cp "infra/example/$$f" "$(dir $(POLICY))$$f"; \
	    echo "seeded $(dir $(POLICY))$$f (ADR 0034: the apps connected to Ghost)"; \
	  fi; \
	done
	@mkdir -p "$(dir $(POLICY))agents"

passwords: ## what is in Ghost's vault, by name; set one with: uv run ghostd passwords set NAME
	@$(UV) run ghostd passwords list

passwords-import: ## once: heka-vault, ~/.config/ghost/env and apps.env into the vault, then the plain files deleted
	@$(UV) run ghostd passwords import

echo-cancel: ## install and start the echo-cancelled mic "ghost.mic" (a user service); without it Ghost hears itself
	infra/pipewire/install.sh

echo-cancel-off: ## stop and remove the echo-cancel service
	infra/pipewire/install.sh --uninstall

##@ Develop: Ghost from this checkout

dev: ## the daemon and the desktop app, logs here; stops the service first; Ctrl+C stops both
	@scripts/dev.sh

dev-rust: ## the same with the Rust daemon (cargo build -p ghostd, target/debug/ghostd) on the same socket
	@GHOST_DAEMON_IMPL=rust scripts/dev.sh

daemon: ## the daemon only, logs here (GHOST_DEBUG_AUDIO=1 make daemon to hear what it hears); Ctrl+C stops it
	@scripts/dev.sh daemon

desktop: ## the desktop app only (Tauri window, hot reload), against whichever daemon owns the socket
	@scripts/dev.sh desktop

desktop-web: ## the same UI in a browser at http://localhost:1420; ?mock=busy and other scenarios need no daemon
	@pnpm dev:desktop

reload: ## restart the daemon that make dev or make daemon runs, after a Python change
	@scripts/dev.sh reload

start: ## the daemon in the background, logging to /tmp/ghostd.log (for scripts and agents)
	@scripts/dev.sh preflight
	@$(load_env) GHOST_LAUNCHER=make-start nohup $(UV) run ghostd run > /tmp/ghostd.log 2>&1 & echo "ghostd started, log: /tmp/ghostd.log"

# Waits until the daemon has exited, so two never share the mic. Ducked volumes are restored on exit.
stop: ## stop this checkout's daemon (make start, make daemon or make dev) and wait for it
	@if ! pkill -INT -f '$(DEV_GHOSTD)'; then echo "ghostd not running"; exit 0; fi; \
	echo "ghostd stopping"; \
	for i in $$(seq 1 30); do pgrep -f '$(DEV_GHOSTD)' >/dev/null || { echo "ghostd stopped"; exit 0; }; sleep 0.5; done; \
	echo "ghostd still running after 15 s; sending TERM"; pkill -TERM -f '$(DEV_GHOSTD)'; \
	for i in $$(seq 1 10); do pgrep -f '$(DEV_GHOSTD)' >/dev/null || { echo "ghostd stopped"; exit 0; }; sleep 0.5; done; \
	echo "ghostd did not stop (pid $$(pgrep -f '$(DEV_GHOSTD)' | tr '\n' ' ')); check it before starting another"; exit 1

restart: stop start ## stop the background daemon, wait for it, start it again

##@ Talk to the running Ghost (either one)

wake: ## simulate the wake word
	@$(UV) run ghostd wake

say: ## type a turn to it: make say TEXT="what needs me?"
	@$(UV) run ghostd say "$(TEXT)"

status: ## its state as JSON
	@$(UV) run ghostd status

check: ## config sanity: models, endpoint, devices, and each app with what your permissions block or ask about
	@$(load_env) $(UV) run ghostd check

devices: ## audio devices as PortAudio sees them
	@$(UV) run ghostd devices

latency: ## per-stage p50/p95 of recent turns: make latency [LAST=20]
	@$(UV) run ghostd latency --last $(or $(LAST),10)

restore-volumes: ## undo music ducking if ghostd died mid-session
	@$(UV) run ghostd restore-volumes

##@ Before you push (CI runs these and more)

ci: lint test typecheck ## lint, test and typecheck, as CI does

lint: ## ruff check and format check
	$(UV) run ruff check .
	$(UV) run ruff format --check .

fmt: ## ruff format
	$(UV) run ruff format .

test: ## the Python test suite
	$(UV) run pytest -q

typecheck: ## the TypeScript apps and packages
	pnpm typecheck

api-schema: ## regenerate packages/api/openapi.json from the pydantic models (then pnpm gen for the TS client)
	$(UV) run python -m ghost_api.export > packages/api/openapi.json

##@ Versions and releases (docs/releasing.md; a version is YYYY.M.N and its bump is a PR)

version: ## the version and the next release's; V=2026.10.1 (or V=next) writes it to every copy
	@if [[ -z "$(V)" ]]; then python3 scripts/version.py; exit; fi; \
	if [[ "$(V)" == next ]]; then git fetch --quiet --tags origin || echo "couldn't fetch tags; counting the local ones"; fi; \
	python3 scripts/version.py set "$(V)"

release: ## on main once the version PR has merged: tag HEAD vYYYY.M.N after checks; pushes nothing
	@python3 scripts/version.py release

##@ The always-on Ghost (the ghostd user service)

# The service runs a fixed build of a commit, never the working tree (scripts/deploy.sh):
# ~/.local/share/ghost/runtime/<commit>/, checked before the restart, rolled back by a link.
# A build carries both daemons until W10 of #39 removes the Python one; IMPL picks.
deploy: ## build HEAD (Python and Rust), check it, switch the service to it and restart: make deploy [IMPL=rust|python]
	@$(no_dev_daemon) GHOST_DAEMON_IMPL=$(IMPL) scripts/deploy.sh deploy

deploy-rollback: ## switch the service back to the build that ran before and restart it
	@$(no_dev_daemon) scripts/deploy.sh rollback

deploy-status: ## which build the service runs, and whether it is running
	@scripts/deploy.sh status

service-on: ## start the service now and at every login
	@$(no_dev_daemon) systemctl --user enable --now ghostd && echo "ghostd service on"

service-off: ## stop the service and keep it from starting at login
	@systemctl --user disable --now ghostd && echo "ghostd service off"

service-logs: ## follow the service's logs
	journalctl --user -u ghostd -f

service-install: ## install the ghostd user unit (make deploy then points it at a build)
	mkdir -p $(HOME)/.config/systemd/user
	install -m 644 infra/systemd/ghostd.service $(HOME)/.config/systemd/user/
	systemctl --user daemon-reload

##@ Performance and wake tuning

# The latency harness (docs/benchmarks/latency-harness.md): scripted conversations through the
# real pipeline, headless, mock GitHub in a temp dir. No mic, no speakers. Spends LLM tokens.
perf: ## latency harness: make perf [SCENARIO=lookup,chain] [RUNS=3] [MODEL=a,b] [ENDPOINT=proxy,openrouter] [ARGS=...]
	@$(load_env) $(UV) run ghostd perf run --runs $(or $(RUNS),3) \
	  $(if $(SCENARIO),--scenario $(SCENARIO)) $(if $(MODEL),--model $(MODEL)) \
	  $(if $(ENDPOINT),--endpoint $(ENDPOINT)) $(ARGS)

perf-micro: ## component micro-benchmarks: STT, Kokoro (and ElevenLabs) first audio, Smart Turn, Jev
	@$(load_env) $(UV) run ghostd perf micro $(ARGS)

perf-llm: ## LLM TTFB per endpoint and model with Ghost's prompt and tools: make perf-llm [ARGS=...]
	@$(load_env) $(UV) run ghostd perf llm $(ARGS)

record: ## record a 4 s wake-phrase clip from ghost.mic and the raw mic: make record NAME=hey-ghost-1
	@scripts/record-clip.sh "$(NAME)" $(or $(SECS),4)

wake-eval: ## what Parakeet hears in .scratch/clips/*.wav and whether the wake matcher fires
	@$(UV) run python docs/benchmarks/scripts/wake_eval.py $(CLIPS)

record-takes: ## guided recording of the wake test set (docs/wake-recording.md): make record-takes [BLOCKS=A,C,F] [ARGS=--dry-run]
	@$(UV) run python scripts/record-wake-takes.py $(if $(BLOCKS),--blocks $(BLOCKS)) $(ARGS)

wake-takes-eval: ## score the wake takes, transcript wake against the trained detector: make wake-takes-eval [TAKES=dir]
	@$(UV) run python docs/benchmarks/scripts/wake_takes_eval.py $(if $(TAKES),--takes $(TAKES)) $(ARGS)

##@ Phone app (apps/mobile: Expo, installed over adb; no EAS, no Play Store)

# Needs the Android SDK and JDK 17 (Gradle does not run on the system default Java 25).
MOBILE := apps/mobile
ANDROID_HOME ?= $(HOME)/Android/Sdk
ANDROID_JAVA_HOME ?= /usr/lib/jvm/java-17-openjdk-amd64
ANDROID_ARCH ?= arm64-v8a
APK := dist/android/ghost.apk

define android_env
export ANDROID_HOME="$(ANDROID_HOME)" JAVA_HOME="$(ANDROID_JAVA_HOME)" PATH="$(ANDROID_HOME)/platform-tools:$$PATH";
endef

define need_phone
if ! adb get-state >/dev/null 2>&1; then \
  echo "No phone over adb. Plug it in, turn on USB debugging (Settings > Developer options),"; \
  echo "accept the prompt on the phone, then check with: adb devices"; exit 1; fi;
endef

# The native project is generated from app.json and package.json (Expo prebuild) and not committed.
$(MOBILE)/android/.prebuilt: $(MOBILE)/app.json $(MOBILE)/package.json
	@$(android_env) cd $(MOBILE) && CI=1 npx expo prebuild --platform android --clean --no-install
	@touch $@

android: $(MOBILE)/android/.prebuilt ## dev build on the USB-connected phone, with Metro and hot reload (Ctrl+C stops Metro)
	@$(android_env) $(need_phone) cd $(MOBILE) && npx expo run:android --variant debug -- -PreactNativeArchitectures=$(ANDROID_ARCH)

android-apk: $(MOBILE)/android/.prebuilt ## standalone APK (JS bundled, debug-signed) into dist/android; no phone needed
	@$(android_env) cd $(MOBILE)/android && ./gradlew assembleRelease -PreactNativeArchitectures=$(ANDROID_ARCH) -q
	@mkdir -p $(dir $(APK)) && cp $(MOBILE)/android/app/build/outputs/apk/release/app-release.apk $(APK)
	@echo "$(APK) ($$(du -h $(APK) | cut -f1))"

android-install: android-apk ## build the standalone APK and install it on the USB-connected phone; runs without the workstation
	@$(android_env) $(need_phone) adb install -r $(APK) && adb shell monkey -p dev.heka.ghost -c android.intent.category.LAUNCHER 1 >/dev/null

mobile-web: ## the phone app in a browser (Expo web), for layout work without a phone
	@cd $(MOBILE) && npx expo start --web

##@ The bundle (ADR 0038: one .deb and one AppImage, the desktop app with ghostd inside)

bundle: ## tauri build: the Rust daemon and the two app binaries in release, then the .deb and the AppImage under target/release/bundle/
	@pnpm tauri build

sidecars: ## only the bundle's binaries, into apps/desktop/src-tauri/binaries/ (tauri build does this itself)
	@scripts/sidecars.sh

##@ Packages of the Python daemon (heka-ghostd; gone with W10 of #39, the bundle replaces them)

build-image: ## build the Docker image that produces the .debs (Tauri, Rust, uv)
	docker build -t heka-ghost-build -f packaging/Dockerfile.build .

deb: ## build heka-ghostd, heka-ghost-desktop and heka-ghost into dist/deb (in Docker)
	docker run --rm -v "$(CURDIR)":/src -v heka-ghost-build-cache:/cache -w /src -e HOST_UID=$(shell id -u) -e HOST_GID=$(shell id -g) heka-ghost-build packaging/build-debs.sh $(ONLY)

deb-test: ## install the .debs into a clean Ubuntu 26.04 container and run ghostd check
	docker run --rm -v "$(CURDIR)/dist/deb":/deb:ro ubuntu:26.04 bash -c 'apt-get update -qq && apt-get install -y -qq /deb/*.deb >/dev/null && ghostd --help; ANTHROPIC_BASE_URL=http://x ghostd check'
