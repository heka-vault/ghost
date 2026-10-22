# Ghost developer entry points. Everything runs on the workstation as the operator.
#
#   make env          render ~/.config/ghost/env from the secrets store (+ settings.env)
#   make run          start ghostd in the foreground (Ctrl+C stops it)
#   make wake         simulate the wake word from another shell
#   make stop         stop a ghostd started with `make run` or `make start`
#   make echo-cancel  install the echo-cancelled mic service (ghost.mic)
#   make android      build the phone app and install it on the USB-connected phone
#
# Secrets never live in this repo: `make run` sources $(ENV_FILE), which is the
# rendered file by default, or ./.env if you created one from .env.example.

SHELL := /bin/bash
.DEFAULT_GOAL := help

ENV_FILE ?= $(if $(wildcard .env),.env,$(HOME)/.config/ghost/env)
SETTINGS := $(HOME)/.config/ghost/settings.env
POLICY := $(HOME)/.config/ghost/policy.toml
UV ?= uv

define load_env
set -a; [[ -r "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a;
endef

.PHONY: help sync test lint fmt check devices env policy run start stop restart wake say status latency perf perf-micro perf-llm api-schema restore-volumes \
        echo-cancel echo-cancel-off service service-logs build-image deb deb-test \
        android android-apk android-install mobile-web

help: ## list targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

sync: ## install/refresh the uv workspace
	$(UV) sync --all-packages

test: ## run the test suite
	$(UV) run pytest -q

lint: ## ruff check + format check
	$(UV) run ruff check .
	$(UV) run ruff format --check .

fmt: ## ruff format
	$(UV) run ruff format .

env: ## render ~/.config/ghost/env from the secrets store; seeds settings.env and policy.toml on first run
	@if [[ ! -e "$(SETTINGS)" ]]; then \
	  mkdir -p "$(dir $(SETTINGS))"; \
	  printf 'GHOST_INPUT_DEVICE=ghost.mic\n' > "$(SETTINGS)"; \
	  echo "seeded $(SETTINGS) (ghost.mic); edit it for non-secret settings"; \
	fi
	@if [[ ! -e "$(POLICY)" ]]; then \
	  mkdir -p "$(dir $(POLICY))"; cp infra/policy.example.toml "$(POLICY)"; \
	  echo "seeded $(POLICY); edit it to set how much Ghost may do on its own"; \
	fi
	infra/systemd/ghost-render-env.sh

policy: ## show the autonomy level Ghost resolves for every operation
	@$(load_env) $(UV) run ghostd check

check: ## config sanity (models, endpoint, devices) using $(ENV_FILE)
	@$(load_env) $(UV) run ghostd check

devices: ## list audio devices as PortAudio sees them
	@$(UV) run ghostd devices

run: ## start ghostd in the foreground using $(ENV_FILE); Ctrl+C stops it
	@$(load_env) exec $(UV) run ghostd run

start: ## start ghostd in the background, log to /tmp/ghostd.log (refuses if one is running)
	@if pgrep -f 'bin/ghostd run$$' >/dev/null; then \
	  echo "ghostd is already running (pid $$(pgrep -f 'bin/ghostd run$$' | tr '\n' ' ')); make stop first"; exit 1; fi
	@$(load_env) nohup $(UV) run ghostd run > /tmp/ghostd.log 2>&1 & echo "ghostd started, log: /tmp/ghostd.log"

# Two daemons on one mic both wake; stop waits until every one has exited.
stop: ## stop any ghostd started by make run/start and wait for it (restores ducked volumes on exit)
	@if ! pkill -INT -f 'bin/ghostd run$$'; then echo "ghostd not running"; exit 0; fi; \
	echo "ghostd stopping"; \
	for i in $$(seq 1 30); do pgrep -f 'bin/ghostd run$$' >/dev/null || { echo "ghostd stopped"; exit 0; }; sleep 0.5; done; \
	echo "ghostd still running after 15 s; sending TERM"; pkill -TERM -f 'bin/ghostd run$$'; \
	for i in $$(seq 1 10); do pgrep -f 'bin/ghostd run$$' >/dev/null || { echo "ghostd stopped"; exit 0; }; sleep 0.5; done; \
	echo "ghostd did not stop (pid $$(pgrep -f 'bin/ghostd run$$' | tr '\n' ' ')); check it before starting another"; exit 1

restart: stop start ## stop ghostd, wait for it, start it again

wake: ## simulate the wake word (through the API socket when ghostd is running)
	@$(UV) run ghostd wake

say: ## type a turn to the running Ghost: make say TEXT="what needs my sign-off"
	@$(UV) run ghostd say "$(TEXT)"

status: ## the running daemon's status as JSON
	@$(UV) run ghostd status

latency: ## per-stage p50/p95 of recent turns from latency.jsonl: make latency [LAST=20]
	@$(UV) run ghostd latency --last $(or $(LAST),10)

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

desktop-dev: ## the desktop app from the checkout (Vite hot reload) against whichever ghostd owns the socket; no .deb needed
	@pnpm tauri dev

desktop-web: ## the same UI in a browser at http://localhost:1420 (Vite proxies the socket); no Rust build needed
	@pnpm dev:desktop

# The phone app (apps/mobile, Expo). Built here, installed over adb; no EAS, no Play Store.
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

api-schema: ## regenerate packages/api/openapi.json from the pydantic models
	$(UV) run python -m ghost_api.export > packages/api/openapi.json

restore-volumes: ## undo music ducking if ghostd died mid-session
	@$(UV) run ghostd restore-volumes

echo-cancel: ## install + start ghost-echo-cancel.service (ghost.mic); no PipeWire restart
	infra/pipewire/install.sh

echo-cancel-off: ## stop and remove the echo-cancel service
	infra/pipewire/install.sh --uninstall

service: ## install ghostd.service as a user unit (does not enable it)
	mkdir -p $(HOME)/.config/systemd/user
	install -m 644 infra/systemd/ghostd.service $(HOME)/.config/systemd/user/
	systemctl --user daemon-reload
	@echo "systemctl --user enable --now ghostd   # when ready"

build-image: ## build the Docker image used to produce the .debs (Tauri + Rust + uv)
	docker build -t heka-ghost-build -f packaging/Dockerfile.build .

deb: ## build heka-ghostd, heka-ghost-desktop and heka-ghost .debs into dist/deb (in Docker)
	docker run --rm -v "$(CURDIR)":/src -v heka-ghost-build-cache:/cache -w /src -e HOST_UID=$(shell id -u) -e HOST_GID=$(shell id -g) heka-ghost-build packaging/build-debs.sh $(ONLY)

deb-test: ## install the .debs into a clean Ubuntu 26.04 container and run ghostd check
	docker run --rm -v "$(CURDIR)/dist/deb":/deb:ro ubuntu:26.04 bash -c 'apt-get update -qq && apt-get install -y -qq /deb/*.deb >/dev/null && ghostd --help; ANTHROPIC_BASE_URL=http://x ghostd check'

service-logs: ## follow ghostd.service logs
	journalctl --user -u ghostd -f
