# Ghost developer entry points. Everything runs on the workstation as the operator.
#
#   make env          render ~/.config/ghost/env from the secrets store (+ settings.env)
#   make run          start ghostd in the foreground (Ctrl+C stops it)
#   make wake         simulate the wake word from another shell
#   make stop         stop a ghostd started with `make run` or `make start`
#   make echo-cancel  install the echo-cancelled mic service (ghost.mic)
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

.PHONY: help sync test lint fmt check devices env policy run start stop wake say status api-schema restore-volumes \
        echo-cancel echo-cancel-off service service-logs build-image deb deb-test

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
	  printf 'GHOST_GITHUB_MOCK=1\nGHOST_INPUT_DEVICE=ghost.mic\n' > "$(SETTINGS)"; \
	  echo "seeded $(SETTINGS) (mock GitHub, ghost.mic); edit it for non-secret settings"; \
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

start: ## start ghostd in the background, log to /tmp/ghostd.log
	@$(load_env) nohup $(UV) run ghostd run > /tmp/ghostd.log 2>&1 & echo "ghostd started, log: /tmp/ghostd.log"

stop: ## stop any ghostd started by make run/start (restores ducked volumes on exit)
	@pkill -INT -f 'bin/ghostd run$$' && echo "ghostd stopping" || echo "ghostd not running"

wake: ## simulate the wake word (through the API socket when ghostd is running)
	@$(UV) run ghostd wake

say: ## type a turn to the running Ghost: make say TEXT="what needs my sign-off"
	@$(UV) run ghostd say "$(TEXT)"

status: ## the running daemon's status as JSON
	@$(UV) run ghostd status

desktop-dev: ## the desktop app from the checkout (Vite hot reload) against whichever ghostd owns the socket; no .deb needed
	@pnpm tauri dev

desktop-web: ## the same UI in a browser at http://localhost:1420 (Vite proxies the socket); no Rust build needed
	@pnpm dev:desktop

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
	docker run --rm -v "$(CURDIR)/dist/deb":/deb:ro ubuntu:26.04 bash -c 'apt-get update -qq && apt-get install -y -qq /deb/*.deb >/dev/null && ghostd --help; ghostd abilities; GHOST_GITHUB_MOCK=1 ANTHROPIC_BASE_URL=http://x ghostd check'

service-logs: ## follow ghostd.service logs
	journalctl --user -u ghostd -f
