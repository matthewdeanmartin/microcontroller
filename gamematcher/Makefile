SHELL := bash
.DEFAULT_GOAL := help

API := api
UI := ui
CARGO ?= cargo
ESP_PYTHON ?= C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

.PHONY: help test test-rust test-web check run web web-dev run-bundle certs firmware flash update gate

help:
	@printf '%s\n' \
	  'make test        Rust tests (app + the framework session module) and a UI type check' \
	  'make check       test + fmt + clippy + production UI build' \
	  'make run         API on http://127.0.0.1:8088 (state in api/.local/state)' \
	  'make web-dev     Angular dev server on http://localhost:4200 (proxies /api to :8088)' \
	  'make run-bundle  build the UI and serve UI + API from :8088' \
	  '' \
	  'Board (Waveshare ESP32-C6-LCD-1.47, MAC ac:eb:e6:1e:13:40; see DEPLOY.md):' \
	  'make certs       issue gamematcher.local from the household CA' \
	  'make firmware    build only; TRANSPORT=http|https|http2 (default http2), GATE=1 for the Sprint 0 gate' \
	  'make flash PORT=COMn   FIRST INSTALL / REPAVE: erases the whole board' \
	  'make update PORT=COMn  app only; refuses unless the board already has our partition table' \
	  'make gate URL=http://<board>   Sprint 0 load + outbound-HTTPS measurement'

test: test-rust test-web

test-rust:
	cd ../miniframework/crates/miniframework && $(CARGO) test --features session session
	cd $(API) && $(CARGO) test
	cd $(API) && $(CARGO) test --features gate gate

test-web:
	cd $(UI) && npx tsc -p tsconfig.app.json --noEmit && npx vitest run

check: test
	cd $(API) && $(CARGO) fmt --check && $(CARGO) clippy --all-targets -- -D warnings
	cd $(API) && $(CARGO) clippy --all-targets --features gate -- -D warnings
	cd $(UI) && npx ng build

run:
	cd $(API) && $(CARGO) run

web-dev:
	cd $(UI) && npx ng serve

web:
	cd $(UI) && npx ng build
	node ../miniframework/tools/bundle-web.mjs $(UI)/dist/gamematcher/browser $(API)/.embuild/web

run-bundle: web
	cd $(API) && $(CARGO) run --features bundled-web

certs:
	bash ../miniframework/tools/certs.sh $(API) gamematcher

TRANSPORT ?= http2
firmware: web
	@case "$(TRANSPORT)" in http|https|http2) ;; *) echo 'TRANSPORT must be http, https or http2' >&2; exit 2;; esac
	features="$(filter-out http,$(TRANSPORT))"; \
	if [ -n "$(GATE)" ]; then features="$${features:+$$features,}gate"; fi; \
	bash tools/build-c6.sh "$$features"

flash:
	@[ -n "$(PORT)" ] || { echo 'Set PORT=COMn (the C6 enumerates as 303A:1001).' >&2; exit 2; }
	"$(ESP_PYTHON)" tools/deploy-c6.py --port "$(PORT)" --replace

update:
	@[ -n "$(PORT)" ] || { echo 'Set PORT=COMn (the C6 enumerates as 303A:1001).' >&2; exit 2; }
	"$(ESP_PYTHON)" tools/deploy-c6.py --port "$(PORT)" --update

gate:
	@[ -n "$(URL)" ] || { echo 'Set URL=http://<board ip>' >&2; exit 2; }
	uv run tools/gate.py --url "$(URL)" --ca $(API)/certs/household-ca.crt $(ARGS)
