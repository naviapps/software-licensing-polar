SDK_TOOLING ?= mise run sdk-tooling --
SWIFT_FLAGS := -Xswiftc -strict-concurrency=complete -Xswiftc -warnings-as-errors

.DEFAULT_GOAL := check
.PHONY: check check-source test-managed check-distribution native native-verify check-header generate-header lint test test-rust test-distribution-tools test-c test-dotnet test-consumer test-installed-consumers format clean

check: check-source test test-consumer

check-source: lint check-header test-rust test-managed

check-distribution: native-verify test-distribution-tools test-c test-dotnet
	python3 -B -m distribution.verify_bindings --verify-swift-pin

native:
	$(SDK_TOOLING) xcframework

native-verify:
	$(SDK_TOOLING) xcframework --verify

check-header:
	$(SDK_TOOLING) c-headers check

generate-header:
	$(SDK_TOOLING) c-headers generate

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
	$(SDK_TOOLING) rust-c-consumers

test-rust:
	cargo test --workspace --locked --offline

test-c:
	$(SDK_TOOLING) python -m distribution.verify_bindings --c-only

test-dotnet:
	$(SDK_TOOLING) python -m distribution.verify_bindings --dotnet-only

test-distribution-tools:
	$(SDK_TOOLING) python -m unittest discover -s distribution/tests -v

test-managed:
	python3 -B -m distribution.verify_bindings --prepare-dotnet
	dotnet run --configuration Release --project bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj

clean:
	$(SDK_TOOLING) clean target .build .native-build build fixtures/consumers/swift/.build \
		bindings/dotnet/src/NaviApps.SoftwareLicensing.Polar/bin \
		bindings/dotnet/src/NaviApps.SoftwareLicensing.Polar/obj \
		bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/bin \
		bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/obj
