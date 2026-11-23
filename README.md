# SoftwareLicensingPolar

Standalone Polar license provider for SoftwareLicensing. Rust owns the HTTPS
transport, protocol validation, cancellation, deadlines and retry decisions;
Swift, C and .NET are thin bindings. The SDK uses Polar's public customer-portal
license endpoints. It never contains a merchant token and does not issue licenses,
create checkouts, migrate customers or manage subscriptions.

## Installation

The Rust crate is `software-licensing-polar`; the Swift product is
`SoftwareLicensingPolar` and the .NET project/package is
`NaviApps.SoftwareLicensing.Polar`. Git dependencies use
immutable full revisions. Swift resolves SoftwareLicensing at the same core pin as
Cargo; a consumer using both must select the same revision. Install at a completed
provider commit, rather than tracking `main`. NuGet packages are local validation
outputs, not a published feed.

| Path                      | Ownership                                            |
| ------------------------- | ---------------------------------------------------- |
| `crates/provider`         | Polar protocol and async Rust transport              |
| `crates/ffi`              | Owned C ABI, header and callback lifecycle tests     |
| `Sources`, `Tests`        | Swift adapter and no-network fixtures                |
| `bindings/dotnet`         | .NET adapter, fixtures and package definition        |
| `fixtures`                | Standalone installed-consumer checks                 |
| `Artifacts`, `ThirdParty` | Source-bound framework and original upstream notices |
| `tools`                   | Pinned shared build, lint and consumer drivers       |

SoftwareLicensing owns secure-record semantics, operation ordering and offline
grace. Hosts own storage namespaces, product policy, scheduling, UI and the choice
of issuer for a new key. Polar validates and deactivates only activations with
source `polar`.

### Swift

Use macOS 26 or later and Swift 6.3. Select this repository's
`SoftwareLicensingPolar` product with the matching SoftwareLicensing dependency.
The source-bound universal framework means Swift consumers need no Rust compiler.

```swift
import SoftwareLicensing
import SoftwareLicensingPolar

let provider = try PolarLicenseProvider(
  configuration: .init(
    activationInstanceName: "Your chosen device label",
    licenseScope: .init(
      organizationID: "<POLAR_ORGANIZATION_UUID>",
      benefitIDs: ["<POLAR_LICENSE_BENEFIT_UUID>"]),
    environment: .sandbox))
```

Replace the placeholder organization/benefit IDs with reviewed identifiers from
one sandbox organization before constructing the provider. They are configuration,
not merchant secrets. The constructor rejects placeholders and does no network I/O.
Use `.production` only with independently reviewed production IDs. Sandbox hosts
must use a separate QA bundle and secure-storage namespace: both environments use
source `polar`, so an issuer string does not identify the environment. Never load
production activations into a sandbox host or reuse its storage in production.
Add the core's `SoftwareLicensingKeychain` product when using macOS secure storage.
Keep existing production namespaces and confidential records intact during changes.
Fixture tests and sandbox trials use their own records.

### Rust

Use a Tokio runtime and `PolarLicenseProvider::new(PolarLicenseConfig { ... })`.
`Environment::Sandbox` or `Environment::Production` is mandatory. `activate`,
`validate`, and `deactivate` are async methods using SoftwareLicensing's common
activation/validation types. `execute_with_cancellation` returns typed mutation
outcome evidence when its cancellation future resolves. Dropping its future stops
local work but cannot return evidence or undo a remote mutation.

### C and .NET

The C header is `crates/ffi/include/software_licensing_polar.h`, with full
`software_licensing_polar_` exports. The Polar C ABI is 3. Pair the Swift and .NET
provider with the pinned SoftwareLicensing ABI 4 runtime. The confidential record
schema remains version 2. Configuration and intent are copied JSON;
results are owned. Copy a result view before destroying its owner. Every non-null
started operation completes exactly once, including after cancellation or early
owner destruction. Completion may run before start returns and runs on a Rust
worker; keep the callback context alive through completion and never unwind across
C. Exclusive handle destruction must not race a call borrowing that handle.

.NET exposes the same configuration and async provider interface. See
[the managed guide](bindings/dotnet/README.md). `tools/bindings.py --prepare-dotnet`
resolves the pinned core source without a sibling path or native rebuild. The
native libraries and original notices must accompany a packaged .NET consumer.

Swift and .NET expose retry and timeout properties as optional overrides.
The bindings omit unset options so Rust applies its defaults for both bindings.
Explicit numbers in existing configuration JSON keep their meaning.

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

Cancellation is local, not a server rollback. Activation/deactivation failures
carry `mutationOutcome: notApplied` or `unknown`. If the server may have accepted a
mutation but its reply was lost or malformed, the provider preserves the unknown
outcome and stops that operation. A connection failure before a request is sent is not
applied. A 4xx response without the matching `Polar-Version` preserves the status
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

## Development and verification

Use the tools pinned by `mise.toml` and `rust-toolchain.toml`. Developer drivers
resolve the immutable `sdk-tooling` revision in `tools/tooling-sources.json`; normal
SDK consumers do not run them. See the [shared setup](https://github.com/naviapps/sdk-tooling#readme).

```sh
mise trust mise.toml
mise install
mise exec -- cargo fetch --locked
mise exec -- make native
mise exec -- make check
```

`make check` verifies native provenance, Rust behavior and cancellation, C ownership,
managed callback fixtures and standalone SwiftPM and NuGet consumers. Both standalone
bindings exercise the packaged Polar provider with the core license manager, including
cancelled activation, typed preflight rejection, failed storage, retry and restart.
Rust protocol tests use local values. The owned loopback transport also exercises
release recovery with the pinned licensing core, from an interrupted journal or a
recorded `429` retry deadline through a confirmed `204` reply. Swift, C and .NET
use invalid or pre-cancelled requests that cannot reach Polar.

The isolated Swift consumer resolves the pinned core without host credentials.
For a private checkout, supply file-only mirror Git configuration as documented by
[sdk-tooling](https://github.com/naviapps/sdk-tooling/blob/main/docs/REFERENCE.md#external-swiftpm-consumers).
For independently installed Rust/C checks, pass a complete core repository or Git
bundle as `--git-dependency-source "naviapps/software-licensing=$dependency"` to
`tools/install-consumers.py`. Use `--evidence` with a new path outside this checkout
to retain the result. This validates local Git delivery.

Windows packages target `win-x64`. On an owned prepared Windows ARM64 worker,
run `python -E -S -B tools/windows.py --architecture x64 --tooling-source PATH
--tooling-revision SHA` with the approved tooling checkout and the absolute path
to the sealed x64 .NET SDK in `DOTNET_BIN`. ARM64-hosted MSVC builds the two x64
DLLs. Rust, C, and .NET fixtures run as x64 processes through Windows emulation.
The driver builds the immutable SoftwareLicensing source separately and accepts
the two NuGet packages together. Accepted packages and evidence are retained
under `.native-build/win-x64/<version>` and, when set, `NAVI_CI_ARTIFACTS`.

GitHub Actions runs the SDK and standalone consumer checks on Xcode 27 and
Windows x64 with the pinned SoftwareLicensing and tooling sources. The macOS
workflow also checks the SDK and its standalone Swift consumer with Xcode 26.6.
CI uses fixture providers. Test actual provider integration separately.

## License

SoftwareLicensingPolar is available under the [MIT License](LICENSE).
Retain the upstream notices in `ThirdParty/` when redistributing the native libraries.

## Upstream contract references

- [Polar API versioning](https://polar.sh/docs/api-reference/versioning)
- [License-key benefits and subscription expiry](https://polar.sh/docs/features/benefits/license-keys)
- [Activation](https://polar.sh/docs/api-reference/customer-portal/license-keys/activate)
- [Validation](https://polar.sh/docs/api-reference/customer-portal/license-keys/validate)
- [Deactivation](https://polar.sh/docs/api-reference/customer-portal/license-keys/deactivate)
- [Upstream schema snapshot](https://github.com/polarsource/polar/blob/433d6bfe53ef5167c7982688d85a114c528235f4/server/polar/license_key/schemas.py)

Review the versioned upstream contract before updating the pinned API version.
