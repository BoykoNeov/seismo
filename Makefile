# Thin wrappers around the commands in CLAUDE.md.
PYTHON ?= python

.PHONY: build test clippy fmt run tui tables check

build:
	cargo build --workspace

test:
	cargo test --workspace

clippy:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

run:
	cargo run -p app

tui:
	cargo run -p tui --release

# Run inside a virtualenv outside the repo with tools/requirements.txt installed.
tables:
	$(PYTHON) tools/gen_tables.py

check: fmt clippy test
