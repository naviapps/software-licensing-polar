SWIFT_FLAGS := -Xswiftc -strict-concurrency=complete -Xswiftc -warnings-as-errors
DOTNET_BIN ?= dotnet

.DEFAULT_GOAL := check
.PHONY: check check-distribution native native-verify lint test test-rust test-tools test-c test-dotnet test-consumer test-installed-consumers format

check: check-distribution test test-consumer

check-distribution: lint native-verify test-rust test-tools test-c test-dotnet

native:
	python3 tools/native.py

native-verify:
	python3 tools/native.py --verify

lint:
	mise exec -- python3 -B tools/quality.py check --dotnet
	cargo clippy --workspace --all-targets --locked --offline -- -D warnings

format:
	mise exec -- python3 -B tools/quality.py format --dotnet

test:
	swift test $(SWIFT_FLAGS)

test-consumer:
	python3 -B tools/consumer.py

test-installed-consumers:
	python3 -B tools/install-consumers.py

test-rust:
	cargo test --workspace --locked --offline

test-c:
	python3 tools/bindings.py --c-only

test-dotnet:
	python3 tools/bindings.py --dotnet-only

test-tools:
	python3 -B -m unittest discover -s tools/tests -v
