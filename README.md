# SoftwareLicensing Polar

A provider for Polar's public customer-portal license API with Rust, Swift and
.NET interfaces. A single Rust implementation uses
[SoftwareLicensing](https://github.com/naviapps/software-licensing) types for activation,
validation, deactivation and failures. SoftwareLicensing owns license state, offline
grace and recovery. The host owns secure storage, commercial policy and the choice
of provider.

## Swift

Add this repository as a SwiftPM dependency at a published Git revision and
select the `SoftwareLicensingPolar` product. It supplies a universal macOS 26+
framework for Apple Silicon and Intel.

```swift
.package(
    url: "https://github.com/naviapps/software-licensing-polar.git",
    revision: "<published-commit-sha>"
)
```

Import `SoftwareLicensingPolar` and pass a configured `PolarLicenseProvider` to
SoftwareLicensing's `LicenseManager`. Configure a separate secure storage namespace
for each environment. See the
[standalone Swift consumer](fixtures/consumers/swift/Sources/PolarConsumer/Consumer.swift)
for a complete example.

## .NET

The `NaviApps.SoftwareLicensing.Polar` package provides a Polar implementation of
`ILicenseProvider` for .NET 10. See the [.NET package guide](bindings/dotnet/README.md)
for building the package, selecting a supported runtime and connecting it to
SoftwareLicensing.

## Rust

Depend on this Git repository at an exact published revision. The Rust crate is
`software-licensing-polar` and runs on the host's Tokio runtime. A host that also
depends on SoftwareLicensing should select the same SoftwareLicensing revision as
the provider's [Cargo manifest](Cargo.toml), so both use the same Rust types.

```toml
[dependencies]
software-licensing-polar = { git = "https://github.com/naviapps/software-licensing-polar.git", rev = "<published-commit-sha>" }
```

Construct a `PolarLicenseProvider` with a `PolarLicenseConfig` and an explicit
`Environment::Sandbox` or `Environment::Production`. The configuration requires
a device label, a Polar organization UUID and a nonempty allowlist of license
benefit UUIDs from the same environment. The constructor validates configuration
without contacting Polar. Keep sandbox and production license records in separate
storage namespaces.

The async `activate`, `validate` and `deactivate` methods accept and return
SoftwareLicensing's public types. `execute_with_cancellation` takes a typed
`Intent` and returns mutation outcome evidence when cancelled. Dropping the future
stops local work and discards that evidence. See
[the Git consumer](fixtures/consumers/rust/src/main.rs) for a complete no-network
example.

## Protocol and security contract

The client sends `Polar-Version: 2026-10` to exactly one selected HTTPS origin:
`https://api.polar.sh` or `https://sandbox-api.polar.sh`. There is no public arbitrary
endpoint override, bearer token, TLS bypass, redirect following or automatic
transport replay. OS certificate verification and system proxies remain enabled.
Responses, per-request time, the total operation and retry delays are bounded.
A response establishes its API version only with a single matching
`Polar-Version` field. Repeated or comma-combined values leave the version
unconfirmed.

Every accepted activation/validation must match the configured organization and a
nonempty allowlist of license-benefit UUIDs. Organization, benefit and activation
identities are canonical lowercase UUID4s. The returned key, license identifier,
activation identifier and activation foreign key are checked. Configuration
validation rejects duplicate or empty benefit scopes before any HTTP request.
The stored issuer is `polar`; `planIdentifier` is the verified benefit UUID. A
validation response with a mismatched key, organization, benefit or activation is
an unexpected provider response. A saved grant can use the core's existing bounded
grace policy from its last successful validation.

The explicit nullable `expires_at` field is required. **A null expiry does not mean
Lifetime:** Polar's subscription keys have no expiry while active and become
revoked when the subscription ends. The host must map allowlisted benefit IDs to
its product plans. Successful validation must still say `granted`. Do not infer
subscription billing periods or migration eligibility from key expiry.

Activate, validate and deactivate use the public
`/v1/customer-portal/license-keys/` endpoints. Validation never sets
`increment_usage`. Deactivation requires the saved activation ID; only its proper
204 response confirms success. A 404 is not proof that a previous deactivation
succeeded. A generic 404 can also indicate a removed API version; it must not be
converted into an authoritative revocation of an existing entitlement. A 403 does
not by itself distinguish expiry, revocation and an exhausted activation limit.
Provider bodies, license keys and arbitrary network error messages are never SDK
failure diagnostics. Error bodies can contain submitted secrets.

### Failure and recovery

Cancellation stops local work. The server may still apply a request it received.
Activation and deactivation failures carry `mutationOutcome: notApplied` or
`unknown`. If the server may have accepted a mutation but its reply was lost or
malformed, the provider preserves the unknown outcome and stops that operation.
A connection failure before sending the request yields `notApplied`. A 4xx
response without the matching `Polar-Version` preserves the status
and retry advice but leaves a mutation outcome unknown. A 429 can authorize a
retry of a mutation only when the response carries the matching version.
Validation can retry 429 and 5xx responses within the configured attempt and
deadline bounds. All retries respect `Retry-After`. Known rejections and their
retry advice survive cancellation or a deadline while waiting. Transport
failures are not replayed.

A local clock outside the supported range leaves clock-independent response
evidence intact. Validation with an expiry requires a usable clock. If the
provider cannot convert an absolute `Retry-After` date to a delay, it returns
the HTTP failure without another attempt during that operation.

A host must preserve SoftwareLicensing's pending activation/deactivation records
until an authoritative resolution is available. Preserve the issuing source and
secure-storage namespace of existing records. Customer and subscription
administration belong to the merchant's operator workflow.

## Development

The repository pins Rust in [rust-toolchain.toml](rust-toolchain.toml). Install
the tools from [mise.toml](mise.toml), then fetch the locked dependencies and run
the local gate:

```sh
mise install
mise exec -- cargo fetch --locked
mise exec -- make check
```

`make check` runs lint, Clippy, Rust and binding tests, and the standalone Swift
consumer. Protocol tests use synthetic inputs and transport tests use loopback
servers. GitHub Actions runs the Swift tests on macOS 26 and 27, and the Rust and
.NET tests on Windows x64.

## Distribution checks

The [distribution plan](sdk-distribution.toml) checks the native build receipts,
C ABI, Swift client and NuGet package for the supported platforms. The shared
macOS distribution workflow also rebuilds the XCFramework and checks the
regenerated bytes with a SwiftPM consumer.

To run the independent Rust consumer against a clean Git checkout,
point SDK Tooling at a complete checkout or bundle of the pinned SoftwareLicensing
repository:

```sh
mise run sdk-tooling -- rust-c-consumers --language rust \
  --git-dependency-source naviapps/software-licensing=/absolute/path/to/software-licensing
```

On Windows, add `--windows-architecture x64` and use the pinned Rust toolchain
and Visual Studio C++ build tools. The fixture checks the resolved Rust dependency
identities and verifies that invalid keys and saved activations fail before
network access.

## License

SoftwareLicensing Polar is available under the [MIT License](LICENSE).
[ThirdParty](ThirdParty) contains upstream notices.
