SHELL := bash
.DEFAULT_GOAL := help

CARGO ?= cargo
PYTHON ?= python
export CARGO

.PHONY: help run migrate-login build test check fmt smoke certs firmware

help:
	@printf '%s\n' \
	  'make run       Start JSON API at http://127.0.0.1:8080 (Angular client: localhost:4200)' \
	  'make migrate-login  Upgrade an existing Rust token account to a username/password' \
	  'make build     Build the desktop server' \
	  'make test      Run Rust tests' \
	  'make check     Check formatting, lint, tests, and HTTP restart smoke' \
	  'make fmt       Format Rust source' \
	  'make smoke     Build and test the real desktop HTTP server' \
	  'make certs     Generate local development TLS certificate files' \
	  'make firmware  Build ESP32-S3 firmware (requires credentials; no flashing)'

run:
	bash scripts/run-local.sh

migrate-login: build
	$(PYTHON) scripts/migrate-login.py

build:
	$(CARGO) build --locked

test:
	$(CARGO) test --locked

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
	"$(MAKE)" smoke

fmt:
	$(CARGO) fmt

smoke: build
	$(PYTHON) scripts/smoke.py

certs:
	bash scripts/dev-certs.sh

firmware:
	bash scripts/build-esp32.sh
