SHELL := bash
.DEFAULT_GOAL := help

API := api
UI := ui
CARGO ?= cargo
ESP_PYTHON ?= C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

.PHONY: help test test-rust test-web check run web web-dev site demo run-bundle certs firmware flash update

help:
	@printf '%s\n' \
	  'make test        Rust tests (app, gate) and the UI type check + unit tests' \
	  'make check       test + fmt + clippy + production UI builds' \
	  'make run         API on http://127.0.0.1:8090 (state in api/.local/state)' \
	  'make web-dev     local app dev server on http://localhost:4200 (proxies /api to :8090)' \
	  'make run-bundle  build the local app and serve UI + API from :8090' \
	  'make site        build the companion site (ui/dist/companion)' \
	  'make demo        build the companion demo (ui/dist/demo)' \
	  '' \
	  'Board (ESP32-S3 N16R8; see DEPLOY.md for which one):' \
	  'make certs       issue party.local from the household CA' \
	  'make firmware    build only; TRANSPORT=http|https|http2 (default http), GATE=1 for the Sprint 0 gate' \
	  'make flash PORT=COMn MAC=xx:..   FIRST INSTALL / REPAVE: erases the whole board' \
	  'make update PORT=COMn MAC=xx:..  app only; refuses unless the board already has our partition table'

test: test-rust test-web

test-rust:
	cd $(API) && $(CARGO) test
	cd $(API) && $(CARGO) test --features gate gate

test-web:
	cd $(UI) && npx tsc -p projects/local/tsconfig.app.json --noEmit && npx tsc -p projects/companion/tsconfig.app.json --noEmit && npx vitest run

check: test
	cd $(API) && $(CARGO) fmt --check && $(CARGO) clippy --all-targets -- -D warnings
	cd $(API) && $(CARGO) clippy --all-targets --features gate -- -D warnings
	cd $(UI) && npx ng build local && npx ng build companion && npx ng build companion --configuration demo

run:
	cd $(API) && $(CARGO) run

web-dev:
	cd $(UI) && npx ng serve local

web:
	cd $(UI) && npx ng build local
	node ../miniframework/tools/bundle-web.mjs $(UI)/dist/local/browser $(API)/.embuild/web

site:
	cd $(UI) && npx ng build companion

demo:
	cd $(UI) && npx ng build companion --configuration demo

run-bundle: web
	cd $(API) && $(CARGO) run --features bundled-web

certs:
	bash ../miniframework/tools/certs.sh $(API) party

TRANSPORT ?= http
firmware: web
	@case "$(TRANSPORT)" in http|https|http2) ;; *) echo 'TRANSPORT must be http, https or http2' >&2; exit 2;; esac
	features="$(filter-out http,$(TRANSPORT))"; \
	if [ -n "$(GATE)" ]; then features="$${features:+$$features,}gate"; fi; \
	bash tools/build-s3.sh "$$features"

flash:
	@[ -n "$(PORT)" ] && [ -n "$(MAC)" ] || { echo 'Set PORT=COMn and MAC=<the board in DEPLOY.md>.' >&2; exit 2; }
	"$(ESP_PYTHON)" tools/deploy-s3.py --port "$(PORT)" --expected-mac "$(MAC)" --replace

update:
	@[ -n "$(PORT)" ] && [ -n "$(MAC)" ] || { echo 'Set PORT=COMn and MAC=<the board in DEPLOY.md>.' >&2; exit 2; }
	"$(ESP_PYTHON)" tools/deploy-s3.py --port "$(PORT)" --expected-mac "$(MAC)" --update
