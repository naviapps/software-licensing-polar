using System.Text.Json;
using NaviApps.SoftwareLicensing;
using NaviApps.SoftwareLicensing.Polar;

static void Check(bool value, string message) { if (!value) throw new System.Diagnostics.UnreachableException(message); }
static async Task<LicenseFailure> Failure(Task task, LicenseErrorCode code)
{
    try { await task.WaitAsync(TimeSpan.FromSeconds(5)); }
    catch (LicenseException error) when (error.Failure.Code == code) { return error.Failure; }
    throw new System.Diagnostics.UnreachableException("Expected credential-free typed failure: " + code);
}
static async Task<LicenseFailure> Cancelled(Task task, LicenseMutationOutcome? outcome = LicenseMutationOutcome.Unknown)
{
    try { await task.WaitAsync(TimeSpan.FromSeconds(5)); }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.Cancelled)
    {
        Check(error.Failure.MutationOutcome == outcome, "Cancellation must retain its native mutation outcome.");
        Check(task.IsFaulted && !task.IsCanceled, "Provider cancellation must remain a typed failure for durable reconciliation.");
        return error.Failure;
    }
    throw new System.Diagnostics.UnreachableException("Expected typed cancellation completion.");
}
const string organization = "11111111-1111-4111-8111-111111111111";
const string benefit = "22222222-2222-4222-8222-222222222222";
const string activationId = "33333333-3333-4333-8333-333333333333";
Check(!JsonSerializer.IsReflectionEnabledByDefault, "The tests require reflection-free JSON.");
var config = new PolarLicenseConfiguration("Owned fixture", new(organization, [benefit]), PolarLicenseEnvironment.Sandbox);
var request = new LicenseActivationRequest("synthetic-key-not-sent");
Check(NativeFailure.Normalize(new TypeInitializationException("fixture", new DllNotFoundException("synthetic-private-diagnostic"))).Failure.Code == LicenseErrorCode.UnsupportedPlatform, "Native initialization failure must use a stable core SDK error.");
Check(NativeFailure.Normalize(new EntryPointNotFoundException("synthetic-private-diagnostic")).Message == LicenseErrorCode.IncompatibleAbiVersion.ToString(), "Native failure diagnostics must not escape into public errors.");

Check(NativeFailure.Normalize(new BadImageFormatException("synthetic-private-diagnostic")).Failure.Code == LicenseErrorCode.IncompatibleAbiVersion, "Invalid native image must not be a persisted record error.");
Check(NativeFailure.Normalize(new LicenseException(LicenseErrorCode.UnsupportedVersion)).Failure.Code == LicenseErrorCode.UnsupportedVersion, "Persisted record errors retain their distinct code.");
Check(NativeFailure.Normalize(new OperationCanceledException("synthetic-private-diagnostic")).Failure.MutationOutcome == LicenseMutationOutcome.Unknown, "An untyped cancellation after entering native code cannot prove that no mutation occurred.");
var describedCancellation = new LicenseException(new LicenseFailure(LicenseErrorCode.Cancelled, 429, ulong.MaxValue, LicenseMutationOutcome.Unknown));
Check(ReferenceEquals(NativeFailure.Normalize(describedCancellation), describedCancellation), "Normalizing a typed native failure must preserve all metadata.");

var immediate = new FakeNative { CompleteDuringStart = true };
using (var provider = new PolarLicenseProvider(config, immediate))
{
    var activation = await provider.ActivateAsync(request);
    Check(activation.ActivationIdentifier == activationId && activation.ActivatedAt == DateTimeOffset.FromUnixTimeSeconds(1700000000), "Core DTO/date framing.");
    Check(immediate.OperationDestroys == 1 && immediate.ResultDestroys == 1, "Completion before handle installation must release each owner once.");
    Check(NativeOperation.OutstandingContexts == 0, "Immediate completion context leaked.");
    using var configuration = JsonDocument.Parse(immediate.Configuration!);
    foreach (var option in new[] { "maximumRequestAttempts", "baseRetryDelay", "maximumRetryDelay", "requestTimeoutSeconds", "operationTimeoutSeconds" })
        Check(!configuration.RootElement.TryGetProperty(option, out _), "Rust supplies the default for " + option);
    Check(configuration.RootElement.GetProperty("environment").GetString() == "sandbox", "The environment is explicit and serialized for Rust.");
    var scope = configuration.RootElement.GetProperty("licenseScope");
    Check(scope.GetProperty("organizationId").GetString() == organization && scope.GetProperty("benefitIds")[0].GetString() == benefit, "Organization and benefit scoping reaches Rust unchanged.");
    Check(activation.Source == "polar" && activation.PlanIdentifier == benefit, "A Polar reply retains its provider source and benefit identity.");
    using var intent = JsonDocument.Parse(immediate.Intent!);
    Check(intent.RootElement.GetProperty("operation").GetString() == "activate", "One operation intent, no managed HTTP framing.");
}
Check(immediate.ProviderDestroys == 1, "Provider owner released once.");

var explicitOverrides = config with
{
    MaximumRequestAttempts = 2,
    BaseRetryDelay = 0.5,
    MaximumRetryDelay = 20,
    RequestTimeoutSeconds = 45,
    OperationTimeoutSeconds = 90
};
var overrides = new FakeNative();
using (var provider = new PolarLicenseProvider(explicitOverrides, overrides))
{
    using var configuration = JsonDocument.Parse(overrides.Configuration!);
    var options = configuration.RootElement;
    Check(options.GetProperty("maximumRequestAttempts").GetInt32() == 2
        && options.GetProperty("baseRetryDelay").GetDouble() == 0.5
        && options.GetProperty("maximumRetryDelay").GetDouble() == 20
        && options.GetProperty("requestTimeoutSeconds").GetDouble() == 45
        && options.GetProperty("operationTimeoutSeconds").GetDouble() == 90,
        "Explicit retry and timeout overrides must reach Rust unchanged.");
}

var production = new FakeNative();
using (var provider = new PolarLicenseProvider(config with { Environment = PolarLicenseEnvironment.Production }, production))
{
    using var configuration = JsonDocument.Parse(production.Configuration!);
    Check(configuration.RootElement.GetProperty("environment").GetString() == "production" && production.Starts == 0, "Production construction sends configuration only, without starting an operation.");
}
var invalidEnvironment = new FakeNative();
foreach (var environment in new[] { default(PolarLicenseEnvironment), (PolarLicenseEnvironment)17 })
{
    try
    {
        using var _ = new PolarLicenseProvider(config with { Environment = environment }, invalidEnvironment);
        throw new System.Diagnostics.UnreachableException("Undefined environment accepted.");
    }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }
}
Check(invalidEnvironment.Configuration is null && invalidEnvironment.Starts == 0, "Undefined environment must fail before native creation.");

const string malformedText = "\uD800";
const string validText = "Owned 値 \U0001FA9F \uFFFD";
var strictText = new FakeNative { CompleteDuringStart = true };
foreach (var malformedConfig in new[]
{
    config with { ActivationInstanceName = malformedText },
    config with { LicenseScope = new(malformedText, [benefit]) },
    config with { LicenseScope = new(organization, [malformedText]) }
})
{
    try
    {
        using var _ = new PolarLicenseProvider(malformedConfig, strictText);
        throw new System.Diagnostics.UnreachableException("Malformed UTF-16 in the provider configuration reached native creation.");
    }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }
}
Check(strictText.Configuration is null, "Malformed configuration text must leave the native adapter untouched.");
using (var provider = new PolarLicenseProvider(config with { ActivationInstanceName = validText }, strictText))
{
    using var configuration = JsonDocument.Parse(strictText.Configuration!);
    Check(configuration.RootElement.GetProperty("activationInstanceName").GetString() == validText,
        "Valid Unicode spelling must reach Rust unchanged.");

    var activation = new LicenseActivation("polar", benefit, DateTimeOffset.FromUnixTimeSeconds(1700000000), request.LicenseKey, activationId);
    Task[] invalidIntents =
    [
        provider.ActivateAsync(new(malformedText)),
        provider.ValidateAsync(activation with { Source = malformedText }),
        provider.ValidateAsync(activation with { PlanIdentifier = malformedText }),
        provider.ValidateAsync(activation with { LicenseKey = malformedText }),
        provider.DeactivateAsync(activation with { ActivationIdentifier = malformedText }),
    ];
    foreach (var intent in invalidIntents)
    {
        var failure = await Failure(intent, LicenseErrorCode.InvalidData);
        Check(failure.MutationOutcome == LicenseMutationOutcome.NotApplied, "Malformed text cannot have started a mutation.");
    }
    Check(strictText.Starts == 0 && NativeOperation.OutstandingContexts == 0,
        "Malformed intent text must fail before starting native work or allocating a foreign context.");
    await provider.ActivateAsync(new(validText));
    using var acceptedIntent = JsonDocument.Parse(strictText.Intent!);
    Check(acceptedIntent.RootElement.GetProperty("licenseKey").GetString() == validText,
        "Valid input must preserve the exact key spelling for Rust normalization.");
}

var preCancelled = new FakeNative();
using (var provider = new PolarLicenseProvider(config, preCancelled))
using (var cancellation = new CancellationTokenSource())
{
    cancellation.Cancel();
    await Cancelled(provider.ActivateAsync(request, cancellation.Token), LicenseMutationOutcome.NotApplied);
    var activation = new LicenseActivation("polar", benefit, DateTimeOffset.FromUnixTimeSeconds(1700000000), "synthetic-key-not-sent", activationId);
    await Cancelled(provider.ValidateAsync(activation, cancellation.Token), LicenseMutationOutcome.NotApplied);
    await Cancelled(provider.DeactivateAsync(activation, cancellation.Token), LicenseMutationOutcome.NotApplied);
    Check(preCancelled.Starts == 0 && NativeOperation.OutstandingContexts == 0, "Pre-cancelled calls must not start native work or allocate a foreign context.");
}

var cancelledBeforeNativeStart = new FakeNative();
using (var cancellation = new CancellationTokenSource())
{
    cancellation.Cancel();
    int completions = 0;
    var operation = new NativeOperation(cancelledBeforeNativeStart, _ => completions++, cancellation.Token);
    operation.Start((IntPtr)1, []);
    await Cancelled(operation.Task, LicenseMutationOutcome.NotApplied);
    Check(cancelledBeforeNativeStart.Starts == 0 && completions == 1 && NativeOperation.OutstandingContexts == 0, "Cancellation during start preparation must report notApplied without allocating a native context.");
}

var deferred = new FakeNative();
var disposed = new PolarLicenseProvider(config, deferred);
var pending = disposed.ActivateAsync(request);
Check(NativeOperation.OutstandingContexts == 1, "Pending callback context must be rooted.");
disposed.Dispose(); disposed.Dispose();
Check(deferred.OperationDestroys == 1 && deferred.ProviderDestroys == 1 && NativeOperation.OutstandingContexts == 1, "Destroy requests cancellation but cannot free the callback context.");
GC.Collect(); GC.WaitForPendingFinalizers();
deferred.Complete(deferred.LastOperation, 1, FakeNative.Cancelled);
await Cancelled(pending);
Check(NativeOperation.OutstandingContexts == 0 && deferred.ResultDestroys == 1, "Completion after disposal must release the retained context/result.");
try { await disposed.ActivateAsync(request); throw new System.Diagnostics.UnreachableException("Disposed provider accepted work."); }
catch (ObjectDisposedException) { }
Check(deferred.Starts == 1, "Disposal prevents new native work.");

var cancelInline = new FakeNative { CompleteDuringCancel = true };
using (var provider = new PolarLicenseProvider(config, cancelInline))
using (var cancellation = new CancellationTokenSource())
{
    var operation = provider.ActivateAsync(request, cancellation.Token);
    cancellation.Cancel();
    await Cancelled(operation);
    Check(cancelInline.Cancels == 1 && cancelInline.OperationDestroys == 1, "Inline cancel completion must not deadlock or destroy a leased native handle early.");
}

var beforeInstall = new FakeNative { CompleteDuringDestroy = true };
var installing = new PolarLicenseProvider(config, beforeInstall);
beforeInstall.OnStart = installing.Dispose;
await Cancelled(installing.ActivateAsync(request));
Check(beforeInstall.OperationDestroys == 1 && beforeInstall.ProviderDestroys == 1 && NativeOperation.OutstandingContexts == 0, "Disposal before handle installation must defer context release until callback.");

var cancellationBeforeInstall = new FakeNative { CompleteDuringCancel = true };
using (var cancellation = new CancellationTokenSource())
using (var provider = new PolarLicenseProvider(config, cancellationBeforeInstall))
{
    cancellationBeforeInstall.OnStart = cancellation.Cancel;
    await Cancelled(provider.ActivateAsync(request, cancellation.Token));
    Check(cancellationBeforeInstall.Cancels == 1 && cancellationBeforeInstall.OperationDestroys == 1, "Cancellation before handle installation is forwarded after installation.");
}

var serverWon = new FakeNative();
using (var cancellation = new CancellationTokenSource())
using (var provider = new PolarLicenseProvider(config, serverWon))
{
    var operation = provider.ActivateAsync(request, cancellation.Token);
    cancellation.Cancel();
    serverWon.Complete(serverWon.LastOperation, 0, FakeNative.Activated);
    Check((await operation).ActivationIdentifier == activationId, "Host cancellation must not discard a successful native activation reply.");
}

for (int i = 0; i < 64; i++)
{
    var race = new FakeNative();
    using var cancellation = new CancellationTokenSource();
    using var provider = new PolarLicenseProvider(config, race);
    var operation = provider.ActivateAsync(request, cancellation.Token);
    await Task.WhenAll(Task.Run(cancellation.Cancel), Task.Run(() => race.Complete(race.LastOperation, 0, FakeNative.Activated)));
    await operation;
    Check(race.OperationDestroys == 1 && race.ResultDestroys == 1, "Completion/cancellation race released an owner more than once.");
}
Check(NativeOperation.OutstandingContexts == 0, "Race contexts leaked.");

foreach (var outcome in new[] { LicenseMutationOutcome.NotApplied, LicenseMutationOutcome.Unknown })
{
    var metadata = new FakeNative();
    using var cancellation = new CancellationTokenSource();
    using var provider = new PolarLicenseProvider(config, metadata);
    var operation = provider.ActivateAsync(request, cancellation.Token);
    cancellation.Cancel();
    var outcomeJson = JsonSerializer.Serialize(outcome, Wire.Type<LicenseMutationOutcome>());
    metadata.Complete(metadata.LastOperation, 1, "{\"code\":\"cancelled\",\"statusCode\":429,\"retryAfterSeconds\":18446744073709551615,\"mutationOutcome\":" + outcomeJson + "}");
    var failure = await Cancelled(operation, outcome);
    Check(failure.StatusCode == 429 && failure.RetryAfterSeconds == ulong.MaxValue, "Native callback cancellation must retain the exact UInt64 retry delay and HTTP status.");
    Check(metadata.OperationDestroys == 1 && metadata.ResultDestroys == 1 && NativeOperation.OutstandingContexts == 0, "Metadata-bearing cancellation must release every native owner once.");
}

var missingOutcome = new FakeNative();
using (var provider = new PolarLicenseProvider(config, missingOutcome))
{
    var operation = provider.ActivateAsync(request);
    missingOutcome.Complete(missingOutcome.LastOperation, 1, "{\"code\":\"cancelled\"}");
    await Cancelled(operation, null);
}

var rateLimited = new FakeNative();
using (var cancellation = new CancellationTokenSource())
using (var provider = new PolarLicenseProvider(config, rateLimited))
{
    var operation = provider.ActivateAsync(request, cancellation.Token);
    cancellation.Cancel();
    rateLimited.Complete(rateLimited.LastOperation, 1, "{\"code\":\"rateLimited\",\"statusCode\":429,\"retryAfterSeconds\":18446744073709551615,\"mutationOutcome\":\"notApplied\"}");
    try { await operation; throw new System.Diagnostics.UnreachableException("Rate limiting must remain a typed failure."); }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.RateLimited)
    {
        Check(error.Failure.StatusCode == 429 && error.Failure.RetryAfterSeconds == ulong.MaxValue && error.Failure.MutationOutcome == LicenseMutationOutcome.NotApplied, "Serialized native failures must retain status, retry delay and mutation outcome.");
    }
    Check(rateLimited.Cancels == 1 && !operation.IsCanceled, "Host cancellation must not erase a confirmed rate-limit rejection.");
}

var nullStart = new FakeNative { NullStart = true };
using (var provider = new PolarLicenseProvider(config, nullStart))
    await Failure(provider.ActivateAsync(request), LicenseErrorCode.InvalidData);
Check(NativeOperation.OutstandingContexts == 0, "Null-start contract failure leaked context.");

var throwingStart = new FakeNative { ThrowOnStart = true };
using (var provider = new PolarLicenseProvider(config, throwingStart))
    await Failure(provider.ActivateAsync(request), LicenseErrorCode.InvalidData);
Check(NativeOperation.OutstandingContexts == 0, "Interop start failure leaked context.");

var cancelledStart = new FakeNative { ThrowCancellationOnStart = true };
using (var provider = new PolarLicenseProvider(config, cancelledStart))
    await Cancelled(provider.ActivateAsync(request), LicenseMutationOutcome.Unknown);
Check(NativeOperation.OutstandingContexts == 0, "Untyped native start cancellation leaked context.");

var throwingCancel = new FakeNative { ThrowOnCancel = true };
using (var cancellation = new CancellationTokenSource())
using (var provider = new PolarLicenseProvider(config, throwingCancel))
{
    var operation = provider.ActivateAsync(request, cancellation.Token);
    cancellation.Cancel();
    Check(NativeOperation.OutstandingContexts == 1, "An unsuccessful cancel must retain context until native completion.");
    throwingCancel.Complete(throwingCancel.LastOperation, 0, FakeNative.Activated);
    await operation;
}

var malformed = new FakeNative();
using (var provider = new PolarLicenseProvider(config, malformed))
{
    var operation = provider.ActivateAsync(request);
    malformed.Complete(malformed.LastOperation, 0, "{\"synthetic-secret-not-in-errors\":");
    await Failure(operation, LicenseErrorCode.UnexpectedProviderResponse);
}
var failedCreate = new FakeNative { InvalidConfiguration = true };
try { using var _ = new PolarLicenseProvider(config, failedCreate); throw new System.Diagnostics.UnreachableException("Invalid native configuration accepted."); }
catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }
Check(failedCreate.ResultDestroys == 1 && failedCreate.ProviderDestroys == 0, "Failed construction owns only its error result.");

if (args.Contains("--native"))
{
    using var configured = new PolarLicenseProvider(explicitOverrides);
    try
    {
        using var _ = new PolarLicenseProvider(config with { LicenseScope = new(organization, []) });
        throw new System.Diagnostics.UnreachableException("Invalid native scope accepted.");
    }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }
    try
    {
        using var _ = new PolarLicenseProvider(config with { ActivationInstanceName = malformedText });
        throw new System.Diagnostics.UnreachableException("Malformed instance name became a different valid name.");
    }
    catch (LicenseException error) when (error.Failure.Code == LicenseErrorCode.InvalidProviderConfiguration) { }
    using var provider = new PolarLicenseProvider(config);
    var wrongSource = new LicenseActivation("owned-invalid-source", benefit, DateTimeOffset.FromUnixTimeSeconds(1700000000), "synthetic-key-not-sent", activationId);
    var rejection = await Failure(provider.DeactivateAsync(wrongSource), LicenseErrorCode.InvalidLicense);
    Check(rejection.MutationOutcome == LicenseMutationOutcome.NotApplied, "Foreign issuer rejection retains its typed preflight outcome.");
    using var cancellation = new CancellationTokenSource();
    cancellation.Cancel();
    await Cancelled(provider.ActivateAsync(new(""), cancellation.Token), LicenseMutationOutcome.NotApplied);
    Check(NativeOperation.OutstandingContexts == 0, "Real native preflight operations leaked contexts.");
    Console.WriteLine("Real native configuration/preflight rejection passed; no License API calls were made.");
}
Console.WriteLine("Managed callback installation, ownership, cancellation/disposal races and DTO contracts passed.");
