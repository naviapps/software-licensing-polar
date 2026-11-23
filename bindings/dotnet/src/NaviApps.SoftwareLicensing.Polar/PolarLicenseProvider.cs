using System.Security.Cryptography;
using System.Text.Json;

namespace NaviApps.SoftwareLicensing.Polar;

/// <summary>Thin async binding to the Rust License API provider. Native Rust owns all
/// HTTP, retry, timeout, cancellation and provider-response handling.</summary>
public sealed class PolarLicenseProvider : ILicenseProvider, IDisposable
{
    private readonly object gate = new();
    private readonly IProviderNative native;
    private readonly ProviderHandle provider;
    private readonly HashSet<NativeOperation> active = [];
    private bool disposed;

    public PolarLicenseProvider(PolarLicenseConfiguration configuration)
        : this(configuration, DefaultNative()) { }

    private static PInvokeProviderNative DefaultNative()
    {
        try { return PInvokeProviderNative.Instance; }
        catch (Exception error) { throw NativeFailure.Normalize(error); }
    }

    internal PolarLicenseProvider(PolarLicenseConfiguration configuration, IProviderNative native)
    {
        ArgumentNullException.ThrowIfNull(configuration);
        this.native = native;
        byte[] bytes;
        try { bytes = JsonSerializer.SerializeToUtf8Bytes(configuration, Wire.Type<PolarLicenseConfiguration>()); }
        catch (Exception error) when (error is JsonException or ArgumentException)
        { throw new LicenseException(LicenseErrorCode.InvalidProviderConfiguration); }
        try
        {
            var pointer = native.Create(bytes, out var error);
            if (pointer == IntPtr.Zero)
            {
                if (error != IntPtr.Zero) native.Consume(error);
                throw new LicenseException(LicenseErrorCode.InvalidProviderConfiguration);
            }
            if (error != IntPtr.Zero)
            {
                native.DestroyProvider(pointer);
                native.Consume(error);
                throw new LicenseException(LicenseErrorCode.InvalidData);
            }
            provider = new ProviderHandle(pointer, native);
        }
        catch (Exception error) { throw NativeFailure.Normalize(error); }
        finally { CryptographicOperations.ZeroMemory(bytes); }
    }

    public async Task<LicenseActivation> ActivateAsync(LicenseActivationRequest request, CancellationToken cancellationToken = default)
    {
        var reply = await Perform(new("activate", LicenseKey: request.LicenseKey), cancellationToken).ConfigureAwait(false);
        return Wire.Reply<LicenseActivation>(reply, "activated", "activation");
    }
    public async Task<LicenseValidationResult> ValidateAsync(LicenseActivation activation, CancellationToken cancellationToken = default)
    {
        var reply = await Perform(new("validate", Activation: activation), cancellationToken).ConfigureAwait(false);
        return Wire.Reply<LicenseValidationResult>(reply, "validated", "validation");
    }
    public async Task DeactivateAsync(LicenseActivation activation, CancellationToken cancellationToken = default)
    {
        var reply = await Perform(new("deactivate", Activation: activation), cancellationToken).ConfigureAwait(false);
        Wire.RequireKind(reply, "deactivated");
    }

    private Task<JsonElement> Perform(PolarIntent intent, CancellationToken token)
    {
        if (token.IsCancellationRequested) throw NativeFailure.Cancelled(LicenseMutationOutcome.NotApplied);
        byte[] bytes;
        try { bytes = JsonSerializer.SerializeToUtf8Bytes(intent, Wire.Type<PolarIntent>()); }
        catch (Exception error) when (error is JsonException or ArgumentException)
        {
            throw new LicenseException(new LicenseFailure(
                LicenseErrorCode.InvalidData, MutationOutcome: LicenseMutationOutcome.NotApplied));
        }
        bool held = false;
        try
        {
            var operation = new NativeOperation(native, Remove, token);
            lock (gate)
            {
                ObjectDisposedException.ThrowIf(disposed, this);
                provider.DangerousAddRef(ref held);
                active.Add(operation);
            }
            operation.Start(provider.DangerousGetHandle(), bytes);
            return operation.Task;
        }
        finally
        {
            if (held) provider.DangerousRelease();
            CryptographicOperations.ZeroMemory(bytes);
        }
    }
    private void Remove(NativeOperation operation) { lock (gate) active.Remove(operation); }

    /// <summary>Prevents new operations and requests cancellation of active ones.
    /// Their contexts remain alive until Rust delivers each completion.</summary>
    public void Dispose()
    {
        NativeOperation[] pending;
        lock (gate)
        {
            if (disposed) return;
            disposed = true;
            pending = active.ToArray();
        }
        foreach (var operation in pending) operation.Destroy();
        provider.Dispose();
    }
}
