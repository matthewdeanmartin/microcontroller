SHELL := bash
.DEFAULT_GOAL := help

APP := apps/housemetrics
CARGO ?= cargo

.PHONY: help test test-rust test-web test-tools fixtures check run web web-dev bundle run-bundle certs firmware flash image-check update probe bench

help:
	@printf '%s\n' \
	  'make test        Rust tests (framework + app) and TypeScript decoder tests' \
	  'make check       test + clippy + fmt check + Angular build' \
	  'make run         housemetrics on http://127.0.0.1:8080 (API only)' \
	  'make web-dev     Angular dev server on http://localhost:4200 (proxies /api to :8080)' \
	  'make run-bundle  build the Angular app and serve UI + API from :8080' \
	  'make certs       issue housemetrics.local certificate from the household CA' \
	  'make firmware    build the ESP32-S2 firmware (no flashing)' \
	  'make flash PORT=COMn   FIRST INSTALL: bootloader + table + app (see apps/housemetrics/DEPLOY_S2.md)' 	  'make image-check  is the built image this board'"'"'s? (board marker, chip, size; no board)' 	  'make update PORT=COMn  app-only update: checks chip, MAC, partition table first; keeps NVS' 	  'make probe ADDRESS=ip  strict live check: TLS, identity, every asset, /ca' \
	  'make bench URL=https://housemetrics.local   connection-mode benchmark (bench/)'

test: test-rust test-web test-tools

test-rust:
	cd crates/miniframework && $(CARGO) test
	cd $(APP) && $(CARGO) test

# The TypeScript decoders are tested against bytes the Rust encoders wrote.
fixtures:
	cd crates/miniframework && $(CARGO) run -q --example fixtures -- ../../web/projects/miniframework-ng/fixtures

# Deploy safety and probe logic (no board, no serial port).
test-tools:
	cd tools/boardsafe && uv run --group dev pytest -q

test-web: fixtures
	cd web && npx vitest run

check: test
	cd crates/miniframework && $(CARGO) fmt --check && $(CARGO) clippy --all-targets -- -D warnings
	cd crates/miniframework && $(CARGO) clippy --all-targets --no-default-features -- -D warnings
	cd crates/miniframework && $(CARGO) test --no-default-features
	cd $(APP) && $(CARGO) fmt --check && $(CARGO) clippy --all-targets -- -D warnings
	cd web && npx ng build housemetrics

run:
	cd $(APP) && $(CARGO) run

web-dev:
	cd web && npx ng serve housemetrics

web:
	cd web && npx ng build housemetrics
	node tools/bundle-web.mjs web/dist/housemetrics/browser $(APP)/.embuild/web

run-bundle: web
	cd $(APP) && $(CARGO) run --features bundled-web

certs:
	bash tools/certs.sh $(APP) housemetrics

firmware: certs web
	bash tools/build-firmware.sh $(APP) housemetrics-esp32

flash:
	@[ -n "$(PORT)" ] || { echo 'Set PORT=COMn (the S2 in download mode: hold BOOT, tap RST).' >&2; exit 2; }
	bash tools/flash.sh $(APP) housemetrics-esp32 "$(PORT)" $(ARGS)

BOARDSAFE := uv run --project tools/boardsafe
REGISTRY := $(APP)/boards.py

image-check:
	$(BOARDSAFE) boardsafe-image --registry $(REGISTRY) --board s2

update:
	@[ -n "$(PORT)" ] || { echo 'Set PORT=COMn (the S2 in download mode: hold BOOT, tap RST).' >&2; exit 2; }
	$(BOARDSAFE) boardsafe-update --registry $(REGISTRY) --board s2 --port "$(PORT)" $(ARGS)

probe:
	@[ -n "$(ADDRESS)" ] || { echo 'Set ADDRESS=<board IP> (or housemetrics.local)' >&2; exit 2; }
	$(BOARDSAFE) boardsafe-probe --registry $(REGISTRY) --board s2 --address "$(ADDRESS)"

bench:
	@[ -n "$(URL)" ] || { echo 'Set URL=https://housemetrics.local (or http://...)' >&2; exit 2; }
	cd bench && uv run fmtbench --url "$(URL)" --ca ../$(APP)/certs/household-ca.crt
