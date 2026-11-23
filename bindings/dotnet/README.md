# Polar licensing for .NET

`NaviApps.SoftwareLicensing.Polar` implements the `ILicenseProvider` contract from
`NaviApps.SoftwareLicensing`. The base package owns reusable lifecycle, secure
storage, persistence and offline policy. This provider owns the separate native
library `software_licensing_polar`; it does not add exports to the base library.

## Configuration

Select the environment explicitly, and supply your Polar organization UUID and a
nonempty allowlist of license-key benefit UUIDs. These public identifiers are not
API tokens. The identifiers below are synthetic configuration examples; replace
them with values from the selected sandbox before provider testing.

```csharp
using NaviApps.SoftwareLicensing.Polar;

using var provider = new PolarLicenseProvider(new PolarLicenseConfiguration(
    "Desktop app",
    new PolarLicenseScope(
        "11111111-1111-4111-8111-111111111111",
        ["22222222-2222-4222-8222-222222222222"]),
    PolarLicenseEnvironment.Sandbox));
// Pass provider to the core LicenseManager. Construction does not activate a key.
```

An uninitialized environment (`default(PolarLicenseEnvironment)`) fails with
`InvalidProviderConfiguration` before reaching native code. The named `Sandbox`
and `Production` values still use the same wire strings. Earlier private .NET
builds assigned `Production` the numeric value zero. Rebuild callers that used
that build, and migrate any separately persisted numeric setting through an
explicit environment selection. Store environment names when persisting settings.

Retry and timeout properties are nullable overrides. Leave them unset to inherit
Rust defaults, or provide explicit numbers to override them. Rebuild applications
using the earlier managed configuration signature.

Rust validates the UUIDs and benefit allowlist, owns fixed production/sandbox HTTPS
endpoints, and sends the versioned Polar customer-portal API requests. It performs
response scoping, bounded body reads, retries, deadlines and cancellation. The
managed adapter has no HTTP client, merchant credential or endpoint override.

The managed adapter rejects unpaired UTF-16 surrogates before JSON encoding
and sends valid Unicode text unchanged. Configuration failures use
`InvalidProviderConfiguration`. Operation input failures use `InvalidData`
with mutation outcome `NotApplied`, since native work has yet to start.

Successful activations retain source `polar`, an activation UUID and the granted
benefit UUID as their plan identifier. `ExpiresAt == null` does not identify a
lifetime purchase: active subscriptions also have no expiry date. Product-to-plan
mapping remains a host responsibility. Keep the issuing source on stored
activations and send them only to their original issuer.

Native completion may happen before start returns. Cancelling or disposing an
operation retains its managed context until completion. Disposal prevents new
operations and requests cancellation of pending work. A confirmed native success
is preserved when host cancellation races with it. Cancellation cannot undo a
server-side activation; retain typed mutation uncertainty for durable recovery.
A generic HTTP 404 is not evidence of successful deactivation.
Mutation failures preserve the HTTP status and retry advice. A 4xx response
without a single matching `Polar-Version` leaves the mutation outcome unknown, and
a matching version is required for automatic retry of a rate-limited mutation.

## Development

Authenticate Git access to this repository, `software-licensing` and the pinned
`sdk-tooling` revision, then prepare a standalone checkout from the repository
root:

```sh
mise trust mise.toml
mise install
mise exec -- cargo fetch --locked
mise exec -- python3 -B tools/bindings.py --prepare-dotnet
mise exec -- dotnet build bindings/dotnet/src/NaviApps.SoftwareLicensing.Polar/NaviApps.SoftwareLicensing.Polar.csproj
```

Preparation resolves the immutable core dependency using Cargo metadata and writes
the ignored local `build/bindings/core.props`. It neither compiles Rust nor requires
a sibling checkout. Re-run it after pins or resolved cache paths change. For
coordinated development, pass the absolute core project as
`-p:SoftwareLicensingProject=...`; immutable acceptance uses the actual pinned
dependency.

Building managed sources does not stage either native library. Platform packaging
tools pack the independent provider and core assets into their respective NuGet
packages. Packed consumers use the ordinary `NaviApps.SoftwareLicensing` package
dependency and need neither Cargo nor the developer bootstrap.

Native ABI mismatch uses `LicenseErrorCode.IncompatibleAbiVersion`;
`UnsupportedVersion` refers to unsupported persisted-record versions. Existing
core records retain their source and instance identities.

## Verification boundaries

The fixture program exercises callback installation, exactly-once release,
cancellation/disposal races and DTO framing with an owned managed seam. Its
`--native` mode uses invalid configuration, invalid sources, empty keys and
pre-cancelled operations, which cannot contact Polar. Rust tests use owned
loopback HTTP fixtures. None establish live provider behavior or migrate customers.

For native local tests, set `SOFTWARE_LICENSING_POLAR_NATIVE_LIBRARY` to the exact
source-matched provider library, and `SOFTWARE_LICENSING_NATIVE_LIBRARY` to the
matching base library. `tools/bindings.py --c-only` and `--dotnet-only` consume
macOS framework executables without rebuilding them. The clean NuGet consumer
resolves both packaged native assets, then exercises cancellation, typed Polar
preflight rejection, failed storage, retry and restart using an in-memory store.

The Windows driver `tools/windows.py` builds `win-x64` packages and runs SDK and
clean NuGet fixtures through x64 emulation on a prepared Windows ARM64 worker.
The isolated consumer restores the provider and its exact paired core package
from an owned local feed. Report Windows target execution, macOS acceptance, and
real provider behavior separately.
