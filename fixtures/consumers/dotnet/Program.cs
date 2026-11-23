using NaviApps.SoftwareLicensing;
using NaviApps.SoftwareLicensing.Polar;

static void Require(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

static async Task<LicenseFailure> ExpectFailure(Task operation, LicenseErrorCode code)
{
    try { await operation.WaitAsync(TimeSpan.FromSeconds(5)); }
    catch (LicenseException error) when (error.Failure.Code == code) { return error.Failure; }
    throw new InvalidOperationException($"Expected the typed {code} failure.");
}

static async Task ExpectCancellation(Task operation)
{
    try { await operation.WaitAsync(TimeSpan.FromSeconds(5)); }
    catch (OperationCanceledException) { return; }
    throw new InvalidOperationException("Expected cancellation.");
}

var configuration = new PolarLicenseConfiguration(
    "Independent consumer",
    new("11111111-1111-4111-8111-111111111111", ["22222222-2222-4222-8222-222222222222"]),
    PolarLicenseEnvironment.Sandbox,
    RequestTimeoutSeconds: 45);
try
{
    using var _ = new PolarLicenseProvider(configuration with { LicenseScope = new("", []) });
    throw new InvalidOperationException("An empty Polar scope was accepted.");
}
catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }

try
{
    using var _ = new PolarLicenseProvider(configuration with { Environment = default });
    throw new InvalidOperationException("An unset Polar environment was accepted.");
}
catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }

using var provider = new PolarLicenseProvider(configuration);
// Rust also rejects this key's length if malformed text ever slips through.
var malformedKey = new string('a', 4096) + '\uD800';
var invalidText = await ExpectFailure(provider.ActivateAsync(new(malformedKey)), LicenseErrorCode.InvalidData);
Require(invalidText.MutationOutcome == LicenseMutationOutcome.NotApplied,
    "Malformed input must fail before reaching the native operation.");
var rejection = new LicenseFailure(LicenseErrorCode.InvalidLicenseKey, MutationOutcome: LicenseMutationOutcome.NotApplied);
foreach (var request in new[] { LicenseActivationRequest.Automatic, new LicenseActivationRequest("") })
{
    var failure = await ExpectFailure(provider.ActivateAsync(request), LicenseErrorCode.InvalidLicenseKey);
    Require(failure == rejection, "Native preflight lost the Polar mutation evidence.");
}

static DateTimeOffset ReadClock() => DateTimeOffset.FromUnixTimeSeconds(2000);

var storage = new InMemoryLicenseStorage();
using (var manager = new LicenseManager(provider, storage, clock: ReadClock))
{
    using var cancellation = new CancellationTokenSource();
    cancellation.Cancel();
    await ExpectCancellation(manager.ActivateAsync(LicenseActivationRequest.Automatic, cancellation.Token));
    Require(storage.SaveCount == 0, "A cancelled activation touched storage.");
    // The keyless request reaches Polar's native preflight after the first save.
    storage.FailOnSave = 2;
    await ExpectFailure(manager.ActivateAsync(LicenseActivationRequest.Automatic), LicenseErrorCode.StorageFailure);
    Require(manager.State.HasPendingPersistence && !manager.State.Plan.IsLicensed && storage.SaveCount == 2,
        "The failed save did not retain the inaccessible pending operation.");
    var recovered = await ExpectFailure(manager.RetryPersistenceAsync(), LicenseErrorCode.InvalidLicenseKey);
    Require(recovered == rejection, "The retry lost the Polar mutation evidence.");
    Require(storage.SaveCount == 3, "Persistence retry performed an unexpected write.");
}

using var reopened = new LicenseManager(provider, storage, clock: ReadClock);
var snapshot = reopened.State;
Require(reopened.InitialRestoreError is null && !snapshot.HasPendingActivation
    && !snapshot.HasPendingPersistence && !snapshot.HasActivation
    && snapshot.LastOperationFailure == rejection,
    "Reopening lost the typed rejection or left a pending activation.");
var refresh = await reopened.RefreshAsync(onlyIfDue: true);
Require(refresh.Outcome == LicenseRefreshOutcome.SkippedNoActivation && storage.SaveCount == 3,
    "Startup refresh acted on a rejected activation.");
Console.WriteLine("Standalone SoftwareLicensingPolar acceptance passed.");

sealed class InMemoryLicenseStorage : ILicenseStorage
{
    private byte[]? record;
    public int SaveCount { get; private set; }
    public int? FailOnSave { get; set; }

    public byte[]? Load() => record?.ToArray();

    public void Save(byte[] candidate)
    {
        SaveCount++;
        if (SaveCount == FailOnSave) throw new IOException();
        record = candidate.ToArray();
    }
}
