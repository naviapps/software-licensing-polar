$metadata = cargo metadata --format-version 1 --locked --filter-platform $env:CARGO_BUILD_TARGET | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$core = @($metadata.packages | Where-Object name -eq 'software-licensing')
if ($core.Count -ne 1) { throw 'Expected one Cargo-locked SoftwareLicensing package.' }
$root = Join-Path (Split-Path -Parent $core[0].manifest_path) '../..'
$project = Join-Path $root 'bindings/dotnet/src/NaviApps.SoftwareLicensing/NaviApps.SoftwareLicensing.csproj'
if (!(Test-Path -LiteralPath $project -PathType Leaf)) { throw 'The locked core source lacks a managed project.' }
dotnet run --project bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj --configuration Release "-p:SoftwareLicensingProject=$project"
