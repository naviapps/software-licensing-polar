SWIFT_FLAGS := -Xswiftc -strict-concurrency=complete -Xswiftc -warnings-as-errors

.DEFAULT_GOAL := check
.PHONY: check check-source test-managed check-distribution native native-verify lint test test-rust test-tools test-c test-dotnet test-consumer test-installed-consumers format

check: check-source test test-consumer

check-source: lint test-rust test-managed

check-distribution: native-verify test-tools test-c test-dotnet

native:
	python3 tools/native.py

native-verify:
	python3 tools/native.py --verify

lint:
	mise run sdk-tooling -- quality check --dotnet
	cargo clippy --workspace --all-targets --locked --offline -- -D warnings

format:
	mise run sdk-tooling -- quality format --dotnet

test:
	swift test $(SWIFT_FLAGS)

test-consumer:
	mise run sdk-tooling -- swift-consumer

test-installed-consumers:
	mise run sdk-tooling -- installed-consumers

test-rust:
	cargo test --workspace --locked --offline

test-c:
	python3 tools/bindings.py --c-only

test-dotnet:
	python3 tools/bindings.py --dotnet-only

test-tools:
	python3 -B -m unittest discover -s tools/tests -v

test-managed:
	python3 -B tools/bindings.py --prepare-dotnet
	dotnet run --configuration Release --project bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj
