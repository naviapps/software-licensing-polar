.DEFAULT_GOAL := check
.PHONY: check lint test format

check: lint test

lint:
	mise run lint
	cargo clippy --all-targets --locked --offline -- -D warnings

test:
	cargo test --locked --offline

format:
	mise run format
