using System.Runtime.InteropServices;
using System.Text.Json;

namespace NaviApps.SoftwareLicensing.Polar;

/// <summary>Owns the foreign context until completion, independently of native handle disposal.</summary>
internal sealed class NativeOperation(IProviderNative native, Action<NativeOperation> onCompleted, CancellationToken token)
{
    private readonly object gate = new();
    private readonly TaskCompletionSource<JsonElement> source = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private static readonly SoftwareLicensingPolarCompletion Completion = Complete;
    private static int contexts;
    private GCHandle context;
    private bool hasContext;
    private bool completed;
    private bool cancellationRequested;
    private bool destructionRequested;
    private OperationHandle? operation;
    private CancellationTokenRegistration registration;
    internal Task<JsonElement> Task => source.Task;
    internal static int OutstandingContexts => Volatile.Read(ref contexts);

    internal void Start(IntPtr provider, byte[] intent)
    {
        var registered = token.UnsafeRegister(static value => ((NativeOperation)value!).Cancel(), this);
        bool abort;
        IntPtr pointer = IntPtr.Zero;
        lock (gate)
        {
            registration = registered;
            abort = cancellationRequested || destructionRequested || token.IsCancellationRequested;
            if (!abort)
            {
                context = GCHandle.Alloc(this);
                hasContext = true;
                Interlocked.Increment(ref contexts);
                pointer = GCHandle.ToIntPtr(context);
            }
        }
        if (abort) { Finish(null, NativeFailure.Cancelled(LicenseMutationOutcome.NotApplied)); return; }
        IntPtr started;
        try { started = native.Start(provider, intent, pointer, Completion); }
        catch (Exception error) { Finish(null, NativeFailure.Normalize(error)); return; }
        // NULL is a contract violation with no callback. Ordinary failures still return
        // a handle and complete exactly once through Completion.
        if (started == IntPtr.Zero) { Finish(null, new LicenseException(LicenseErrorCode.InvalidData)); return; }
        var installed = new OperationHandle(started, native);
        bool dispose;
        bool cancel;
        lock (gate)
        {
            dispose = completed || destructionRequested;
            cancel = cancellationRequested;
            if (!dispose) operation = installed;
        }
        if (dispose) installed.Dispose();
        else if (cancel) Cancel();
    }

    private static void Complete(IntPtr pointer, IntPtr result)
    {
        // Native guarantees one callback for a non-null start. The GCHandle remains
        // live even after cancel/destroy; no managed exception may cross this boundary.
        NativeOperation? state = null;
        try
        {
            state = (NativeOperation)GCHandle.FromIntPtr(pointer).Target!;
            state.Receive(result);
        }
        catch
        {
            state?.Finish(null, new LicenseException(LicenseErrorCode.UnexpectedProviderResponse));
        }
    }

    private void Receive(IntPtr result)
    {
        JsonElement? value = null;
        Exception? failure = null;
        try { value = native.Consume(result); }
        catch (LicenseException error) { failure = error; }
        catch { failure = new LicenseException(LicenseErrorCode.UnexpectedProviderResponse); }
        Finish(value, failure);
    }

    private void Finish(JsonElement? result, Exception? failure)
    {
        OperationHandle? owned;
        CancellationTokenRegistration registered;
        GCHandle released = default;
        bool releaseContext;
        lock (gate)
        {
            if (completed) return;
            completed = true;
            owned = operation;
            operation = null;
            registered = registration;
            releaseContext = hasContext;
            if (releaseContext) { released = context; hasContext = false; }
        }
        // Unregister never waits for a cancellation callback that may be in a native
        // call completing us. SafeHandle leases keep concurrent cancel/dispose safe.
        registered.Unregister();
        try { owned?.Dispose(); }
        finally
        {
            if (releaseContext) { released.Free(); Interlocked.Decrement(ref contexts); }
            onCompleted(this);
        }
        // Rust decides whether cancellation or a completed server response wins. Do not
        // discard a successful activation merely because the host token is now cancelled.
        // Typed cancellation metadata lets the core manager reconcile durable mutations.
        if (failure is not null) source.TrySetException(NativeFailure.Normalize(failure));
        else if (result is { } value) source.TrySetResult(value);
        else source.TrySetException(new LicenseException(LicenseErrorCode.UnexpectedProviderResponse));
    }

    private void Cancel()
    {
        OperationHandle? current;
        lock (gate)
        {
            if (completed) return;
            cancellationRequested = true;
            current = operation;
        }
        try { current?.Cancel(); }
        catch
        {
            // Cancellation is best effort. Never unwind into CancellationTokenSource
            // or free the foreign context while its native completion is still pending.
        }
    }

    internal void Destroy()
    {
        OperationHandle? current;
        lock (gate)
        {
            if (completed) return;
            destructionRequested = true;
            cancellationRequested = true;
            current = operation;
            operation = null;
        }
        // Destroy requests cancellation; only the eventual callback frees the context.
        current?.Dispose();
    }
}
