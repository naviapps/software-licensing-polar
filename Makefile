SDK_TOOLING ?= mise run sdk-tooling --
SWIFT_FLAGS := -Xswiftc -strict-concurrency=complete -Xswiftc -warnings-as-errors

.DEFAULT_GOAL := check
.PHONY: check check-source test-managed check-distribution native native-verify lint test test-rust test-fixtures test-c test-dotnet test-consumer test-installed-consumers format

check: check-source test test-consumer

check-source: lint test-rust test-managed

check-distribution: native-verify test-fixtures test-c test-dotnet

native:
	$(SDK_TOOLING) native

native-verify:
	$(SDK_TOOLING) native --verify

lint:
	$(SDK_TOOLING) quality check --dotnet
	cargo clippy --workspace --all-targets --locked --offline -- -D warnings

format:
	$(SDK_TOOLING) quality format --dotnet

test:
	swift test $(SWIFT_FLAGS)

test-consumer:
	$(SDK_TOOLING) swift-consumer

test-installed-consumers:
	$(SDK_TOOLING) installed-consumers

test-rust:
	cargo test --workspace --locked --offline

test-c:
	$(SDK_TOOLING) python -m fixtures.distribution.bindings --c-only

test-dotnet:
	$(SDK_TOOLING) python -m fixtures.distribution.bindings --dotnet-only

test-fixtures:
	$(SDK_TOOLING) python -m unittest discover -s fixtures/distribution/tests -v

test-managed:
	python3 -B -m fixtures.distribution.bindings --prepare-dotnet
	dotnet run --configuration Release --project bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj
