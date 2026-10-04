SHELL := bash
.DEFAULT_GOAL := help
.PHONY: help run build firmware deploy ports fmt test
help:
	@printf '%s\n' 'make run       Desktop website: http://127.0.0.1:8080' 'make firmware  Build ESP32-S2 firmware' 'make ports     List attached serial devices' 'make deploy PORT=COM6  Update an already-provisioned S2' 'make test      Run job and protocol tests'
run:
	cargo run --locked
build:
	cargo build --locked
firmware:
	bash scripts/build-esp32.sh
deploy:
	bash scripts/deploy.sh "$(PORT)"
ports:
	python -m serial.tools.list_ports -v
fmt:
	cargo fmt
test:
	cargo test --locked
