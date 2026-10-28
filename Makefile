.PHONY: help lint test build fmt run

help:
	@printf '%s\n' \
	  'make lint   Check formatting and run Clippy' \
	  'make test   Run the test suite' \
	  'make build  Build and sign dist/MCP Manager.app' \
	  'make fmt    Format Rust source files' \
	  'make run    Run the development app'

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings

test:
	cargo test --locked

build:
	bash scripts/package.sh

fmt:
	cargo fmt

run:
	cargo run --locked
