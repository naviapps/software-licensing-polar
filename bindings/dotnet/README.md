# Polar licensing for .NET

`NaviApps.SoftwareLicensing.Polar` implements the `ILicenseProvider` contract from
`NaviApps.SoftwareLicensing`. See the
[provider contract](https://github.com/naviapps/software-licensing-polar#protocol-and-security-contract)
for license scope, retries and recovery.

## Installation

Use .NET 10 with a matching package runtime. Use `win-x64` with x64 .NET on
Windows, including on Windows 11 ARM64 under emulation. On macOS 26 or later,
use `osx-arm64` on Apple Silicon or `osx-x64` on Intel.

Download the `software-licensing-polar-nuget-<revision>-<attempt>` artifact
from a successful [SDK distribution run](https://github.com/naviapps/sdk-tooling/actions/workflows/check-sdk-distribution.yml).
Check each archive's SHA-256 against its `.nupkg.json` receipt. Copy the provider
package from the artifact root and Software Licensing from `dependencies/` into
one local feed. Both packages cover all three runtimes, and the provider requires
the exact base version from the same artifact.

In an empty directory, create a console project and use the exact provider version
from the artifact. Restore with a fresh cache each time you switch between packages
with the same version.

```sh
dotnet new console --framework net10.0 --no-restore
dotnet package add NaviApps.SoftwareLicensing.Polar --version "[0.1.0]" --no-restore
dotnet restore --source "/path/to/verified/packages" --packages .nuget-packages -p:UseAppHost=false
```

To try the package, select the artifact's revision in the
[SDK repository](https://github.com/naviapps/software-licensing-polar) and copy
`fixtures/consumers/dotnet/Program.cs` to your project. Run
`dotnet run --no-restore -p:UseAppHost=false` to check cancellation and storage
recovery with synthetic inputs.

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

## .NET behavior

The binding rejects unpaired UTF-16 surrogates before JSON encoding and preserves
valid Unicode. Configuration errors use `InvalidProviderConfiguration`. Invalid
operation inputs use `InvalidData` with mutation outcome `NotApplied`.

`LicenseErrorCode.IncompatibleAbiVersion` identifies a native ABI mismatch.
`UnsupportedVersion` identifies a persisted-record version the core cannot read.

Native completion can happen before start returns. Cancellation and disposal
retain the managed context until completion. Disposing the provider prevents new
operations and requests cancellation of pending work. A confirmed native success
survives a cancellation race. Keep pending records when the mutation outcome is
unknown, as described in the
[failure and recovery contract](https://github.com/naviapps/software-licensing-polar#failure-and-recovery).

For building or verifying a package, see the
[maintainer guide](https://github.com/naviapps/software-licensing-polar#distribution-checks).
