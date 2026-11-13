# Ghost's entry points. `make` alone lists them, grouped by what you are doing.
#
# Working on Ghost:
#   make prereqs   what this machine is missing to build Ghost, and the command that installs it
#   make setup     once per machine: the build, the models, Ghost's folder
#   make dev       the daemon and the desktop app from this checkout, both logging to this
#                  terminal; the UI reloads on save, and Ctrl+C stops both
#   make reload    in another terminal after a Rust change: rebuilds and restarts the daemon,
#                  not the app
#   make say TEXT="what needs me?"   types a turn to it; or say "Hey Ghost"
#
# Ghost is one Cargo workspace (ADR 0039): the daemon, crates/ghostd and the crates beside it,
# and the desktop app's shell. It ships as one desktop bundle with the daemon inside
# (ADR 0038): `make bundle` makes the .deb and the AppImage, and the installed app runs ghostd
# while you are logged in. The ghostd user service is the headless mode, running a fixed
# build of a commit that `make deploy` made. Only one Ghost may have the microphone, so
# `make dev` stops the service and `make service-on` starts it again; the app adopts a daemon
# already on the socket. The Python that is left is tools: tools/ (the version script and the
# scripts' tests) and tools/oracle (the model wrappers the Rust daemon is measured against,
# the aiortc far end, the wake tooling), each a uv project.
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
ORACLE := $(UV) run --project tools/oracle
export ENV_FILE UV

# This checkout's daemon and no other: the service's ghostd runs from a build elsewhere.
GHOSTD := target/debug/ghostd
DEV_GHOSTD := $(CURDIR)/target/debug/ghostd run$$

# GHOST_SETTINGS_LOADED: ghostd reads settings.env itself when started by hand; the targets
# load $(ENV_FILE) (which may be a ./.env in its place) and say so, so it is read once.
define load_env
set -a; [[ -r "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a; export GHOST_SETTINGS_LOADED=1;
endef

define no_dev_daemon
if pgrep -f '$(DEV_GHOSTD)' >/dev/null; then \
  echo "This checkout's ghostd is running; stop it first (Ctrl+C in make dev, or make stop)."; exit 1; fi;
endef

# The checkout's daemon, compiled first (quick when nothing changed), with the settings loaded.
define ghostd
cargo build -q -p ghostd || exit 1; $(load_env) $(GHOSTD)
endef

.PHONY: help prereqs setup sync env passwords passwords-import echo-cancel echo-cancel-off demo-audio \
        dev daemon desktop desktop-web reload start stop restart \
        wake say status check latency restore-volumes \
        lint fmt test deny typecheck contract web ci api-schema version release screenshots \
        deploy deploy-rollback deploy-status service-on service-off service-logs service-install \
        perf perf-micro perf-llm perf-providers perf-hear perf-turns record wake-eval record-takes wake-takes-eval wake-check-replay wake-dynamics \
        wake-train wake-detectors wake-use wake-rollback \
        android android-apk android-install mobile-web \
        bundle sidecars onnxruntime models

help: ## list the targets
	@awk 'BEGIN {FS = ":.*## "} /^##@ / {printf "\n%s\n", substr($$0, 5)} /^[a-z][a-z-]*:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

##@ Set up, once per machine (safe to repeat)

prereqs: ## check the system packages, Rust, Node, pnpm and uv, and say the command that installs what is missing (never runs sudo)
	@scripts/prereqs.sh

setup: ## prereqs, then the daemon built, the Python tools synced (make sync adds the oracle), the runtime and the models fetched, pnpm install, Ghost's folder, and ghostd check
	@scripts/prereqs.sh
	cargo build -p ghostd -p ghost-core --bins
	$(UV) sync
	@$(MAKE) --no-print-directory onnxruntime
	@$(MAKE) --no-print-directory models
	pnpm install
	@$(MAKE) --no-print-directory env
	@$(ghostd) check || true
	@echo; echo "Ready: make dev starts the daemon and the desktop app from this checkout."
	@echo "Optional: make echo-cancel, so Ghost doesn't hear itself on open speakers (README.md, \"Sound\")."

sync: ## install or refresh the Python tools: the root workspace and the oracle
	$(UV) sync
	$(UV) sync --project tools/oracle

onnxruntime: ## fetch ONNX Runtime 1.24.4 into ~/.cache/ghost/onnxruntime/ once, where the daemon loads it from (the bundle carries its own)
	@scripts/onnxruntime.sh >/dev/null

models: ## fetch the model files into ~/.cache/ghost/ once, pinned: Silero, Smart Turn, the wake front end (the bundle carries these), Parakeet (2.5 GB) and Kokoro (340 MB)
	@scripts/models.sh >/dev/null
	@scripts/models.sh parakeet >/dev/null
	@scripts/models.sh kokoro >/dev/null

env: ## seed settings.env, policy.toml and Ghost's folder on first run (passwords: make passwords)
	@if [[ ! -e "$(SETTINGS)" ]]; then \
	  mkdir -p "$(dir $(SETTINGS))"; cp infra/example/settings.env "$(SETTINGS)"; \
	  echo "seeded $(SETTINGS); non-secret settings only (passwords: make passwords)"; \
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

passwords: ## what is in Ghost's vault, by name; set one with: target/debug/ghostd passwords set NAME
	@$(ghostd) passwords list

passwords-import: ## once: heka-vault, ~/.config/ghost/env and apps.env into the vault, then the plain files deleted
	@$(ghostd) passwords import

echo-cancel: ## optional: the echo-cancelled mic "ghost.mic" (a user service), so Ghost doesn't hear itself on open speakers
	infra/pipewire/install.sh

echo-cancel-off: ## stop and remove the echo-cancel service
	infra/pipewire/install.sh --uninstall

demo-audio: ## for a demo on a call: a mic "Ghost and your microphone" with Ghost's voice mixed in; Ctrl+C removes it
	@scripts/demo-audio.sh

##@ Develop: Ghost from this checkout

dev: ## the daemon (cargo build -p ghostd) and the desktop app, logs here; stops the service first; Ctrl+C stops both
	@scripts/dev.sh

daemon: ## the daemon only, logs here (GHOST_DEBUG_AUDIO=1 make daemon to hear what it hears); Ctrl+C stops it
	@scripts/dev.sh daemon

desktop: ## the desktop app only (Tauri window, hot reload), against whichever daemon owns the socket
	@scripts/dev.sh desktop

desktop-web: ## the same UI in a browser at http://localhost:1420; ?mock=busy and other scenarios need no daemon
	@pnpm dev:desktop

screenshots: ## the README's screenshots from the mock UI (make desktop-web running; Chrome and ImageMagick): make screenshots [SHOTS="home apps"]
	@scripts/screenshots.sh $(SHOTS)

reload: ## rebuild and restart the daemon that make dev or make daemon runs, after a Rust change
	@scripts/dev.sh reload

start: ## the daemon in the background, logging to /tmp/ghostd.log (for scripts and agents)
	@scripts/dev.sh preflight
	@cargo build -q -p ghostd
	@$(load_env) GHOST_LAUNCHER=make-start nohup $(CURDIR)/$(GHOSTD) run > /tmp/ghostd.log 2>&1 & echo "ghostd started, log: /tmp/ghostd.log"

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
	@$(ghostd) wake

say: ## type a turn to it: make say TEXT="what needs me?"
	@$(ghostd) say "$(TEXT)"

status: ## its state as JSON
	@$(ghostd) status

check: ## config sanity: the models and the runtime, the model connection, devices, and each app with what your permissions block or ask about
	@$(ghostd) check

latency: ## per-stage p50/p95 of recent turns: make latency [LAST=20]
	@$(ghostd) latency --last $(or $(LAST),10)

restore-volumes: ## undo music ducking if ghostd died mid-session
	@$(ghostd) restore-volumes

##@ Before you push (CI runs these and more)

ci: lint test deny typecheck contract web ## everything CI runs but the bundle: lint, test, deny, typecheck, the API contract, the web tests and build

lint: ## rustfmt and clippy over the workspace, then ruff over the Python tools and scripts
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	$(UV) run ruff check .
	$(UV) run ruff format --check .

fmt: ## rustfmt and ruff format
	cargo fmt --all
	$(UV) run ruff format .

test: ## every Rust test (cargo nextest), then the tools' pytest
	cargo nextest run --workspace
	$(UV) run --project tools pytest -q

deny: ## cargo deny: a known vulnerability or a license outside deny.toml fails
	cargo deny check

typecheck: ## the TypeScript apps and packages
	pnpm typecheck

contract: ## openapi.json current from the Rust types, and the TS client current from it
	cargo run -q -p ghost-api --bin ghost-api-export --locked | diff -u packages/api/openapi.json -
	pnpm gen:check

web: ## the TypeScript tests (vitest) and the desktop's production build
	pnpm test
	pnpm build

api-schema: ## regenerate packages/api/openapi.json from the Rust types (crates/ghost-api), then the TS client
	cargo run -q -p ghost-api --bin ghost-api-export > packages/api/openapi.json
	pnpm gen

##@ Versions and releases (docs/releasing.md; a version is YYYY.M.N and its bump is a PR)

version: ## the version and the next release's; V=2026.10.1 (or V=next) writes it to every copy
	@if [[ -z "$(V)" ]]; then python3 tools/version.py; exit; fi; \
	if [[ "$(V)" == next ]]; then git fetch --quiet --tags origin || echo "couldn't fetch tags; counting the local ones"; fi; \
	python3 tools/version.py set "$(V)"

release: ## on main once the version PR has merged: tag HEAD vYYYY.M.N after checks; pushes nothing
	@python3 tools/version.py release

##@ The always-on Ghost (the ghostd user service, the headless mode)

# The service runs a fixed build of a commit, never the working tree (scripts/deploy.sh):
# ~/.local/share/ghost/runtime/<commit>/, checked before the restart, rolled back by a link.
deploy: ## build HEAD (cargo build --release -p ghostd), check it, switch the service to it and restart
	@$(no_dev_daemon) scripts/deploy.sh deploy

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
# real pipeline (crates/ghost-voice, ADR 0039), headless, the real local models and the mock
# GitHub app in a temp dir, a scripted model. No mic, no speakers, no tokens spent.
perf: ## latency harness: make perf [SCENARIO=lookup,chain] [RUNS=3] [ARGS=--no-jev ...]
	@cargo build --release -q -p ghostd --bin ghostd -p ghost-core --bin ghost-mock-github
	@$(load_env) target/release/ghostd perf run --runs $(or $(RUNS),3) \
	  $(if $(SCENARIO),--scenario $(SCENARIO)) $(ARGS)

perf-micro: ## component micro-benchmarks: STT, Kokoro first audio, Smart Turn, Jev
	@cargo build --release -q -p ghostd --bin ghostd
	@$(load_env) target/release/ghostd perf micro $(ARGS)

perf-llm: ## model and endpoint TTFB with Ghost's prompt and tools (a few thousand cached tokens a call): make perf-llm [ARGS=...]
	@cargo build --release -q -p ghostd --bin ghostd -p ghost-core --bin ghost-mock-github
	@$(load_env) target/release/ghostd perf llm $(ARGS)

perf-providers: ## the provider check (ADR 0056) on every provider with a key in the environment, compared: make perf-providers [PROVIDERS="proxy anthropic:ANTHROPIC_API_KEY openrouter:OPENROUTER_API_KEY openrouter-openai"] [ARGS=--samples 5]
	@cargo build --release -q -p ghostd --bin ghostd
	@target/release/ghostd perf providers $(or $(PROVIDERS),proxy anthropic openrouter openrouter-openai openai) $(ARGS)

perf-hear: ## how well Ghost hears (docs/benchmarks/hear-harness.md): make perf-hear [TAKES=latest|dir] [BASELINE=dir/results.json] [ARGS=--show-text ...]
	@cargo build --release -q -p ghostd --bin ghostd
	@$(load_env) target/release/ghostd perf hear $(if $(TAKES),--takes $(TAKES)) \
	  $(if $(BASELINE),--baseline $(BASELINE)) $(ARGS)

perf-turns: ## when Ghost would take a turn for over, on the same takes (docs/benchmarks/hear-harness.md): make perf-turns [TAKES=latest|dir] [ARGS=--set turn_confident_prob=0.95 ...]; with REALTIME_KEY_ENV=VAR the realtime voice's first sound and relay instead (docs/realtime-voice.md)
	@cargo build --release -q -p ghostd --bin ghostd
	@$(if $(REALTIME_KEY_ENV),target/release/ghostd perf realtime --key-env $(REALTIME_KEY_ENV) $(ARGS),$(load_env) target/release/ghostd perf turns $(if $(TAKES),--takes $(TAKES)) $(ARGS))

record: ## record a 4 s wake-phrase clip from ghost.mic and the raw mic: make record NAME=hey-ghost-1
	@scripts/record-clip.sh "$(NAME)" $(or $(SECS),4)

wake-eval: ## what Parakeet hears in .scratch/clips/*.wav and whether the wake matcher fires (the oracle)
	@$(ORACLE) python -I docs/benchmarks/scripts/wake_eval.py $(CLIPS)

record-takes: ## guided recording of the wake test set (docs/wake-recording.md): make record-takes [BLOCKS=A,C,F] [ARGS=--dry-run]
	@$(ORACLE) python -I scripts/record-wake-takes.py $(if $(BLOCKS),--blocks $(BLOCKS)) $(ARGS)

wake-takes-eval: ## score the wake takes, transcript wake against the trained detector (the oracle): make wake-takes-eval [TAKES=dir]
	@$(ORACLE) python -I docs/benchmarks/scripts/wake_takes_eval.py $(if $(TAKES),--takes $(TAKES)) $(ARGS)

wake-train: ## retrain the "hey ghost" detector on your labelled wake takes, on this computer (ADR 0065, docs/wake-takes.md): make wake-train [ARGS="--unlabelled-accepted-are-me"]
	@$(UV) sync -q --project tools/wake-train
	@$(UV) run -q --project tools/wake-train python -I -m wake_train $(ARGS)

wake-detectors: ## the retrained detectors beside the shipped one, with their numbers; * marks the one in use
	@cargo run -q -p ghostd -- detector list

wake-use: ## hear with a retrained detector: make wake-use NAME=operator-20261113-0900 (restart Ghost after)
	@cargo run -q -p ghostd -- detector use $(NAME)

wake-rollback: ## back to the shipped detector (restart Ghost after)
	@cargo run -q -p ghostd -- detector use shipped

wake-check-replay: ## the detector and its check of a weak hit over labelled streams, before and after (the oracle): make wake-check-replay ARGS="--stream f.wav:pos@12.3 --stream g.wav:neg"
	@$(ORACLE) python -I docs/benchmarks/scripts/wake_check_replay.py $(ARGS)

wake-dynamics: ## compression, normalisation, AGC and filters before the wake detector and its check, over the labelled streams (the oracle): make wake-dynamics ARGS="--only base,c4-fast"
	@$(ORACLE) python -I docs/benchmarks/scripts/wake_dynamics.py $(ARGS)

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

bundle: ## tauri build: the daemon, the two app binaries, the ONNX Runtime and the model files in release, then the .deb and the AppImage under target/release/bundle/
	@pnpm tauri build

sidecars: ## only the bundle's binaries, the runtime library and the model files, into apps/desktop/src-tauri/ (tauri build does this itself)
	@scripts/sidecars.sh
