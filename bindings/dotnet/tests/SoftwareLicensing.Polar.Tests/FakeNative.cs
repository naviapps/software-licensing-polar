using System.Collections.Concurrent;
using System.Text.Json;
using NaviApps.SoftwareLicensing.Polar;

sealed class FakeNative : IProviderNative
{
    private sealed class Entry(IntPtr context, SoftwareLicensingPolarCompletion completion)
    {
        internal readonly IntPtr Context = context;
        internal readonly SoftwareLicensingPolarCompletion Completion = completion;
        internal int Completed;
        internal int Destroyed;
    }
    private readonly ConcurrentDictionary<IntPtr, Entry> operations = new();
    private readonly ConcurrentDictionary<IntPtr, (uint Code, byte[] Bytes)> results = new();
    private long sequence = 1;
    internal bool CompleteDuringStart;
    internal bool CompleteDuringCancel;
    internal bool CompleteDuringDestroy;
    internal bool NullStart;
    internal bool InvalidConfiguration;
    internal bool ThrowOnStart;
    internal bool ThrowCancellationOnStart;
    internal bool ThrowOnCancel;
    internal Action? OnStart;
    internal int Starts;
    internal int Cancels;
    internal int OperationDestroys;
    internal int ProviderDestroys;
    internal int ResultDestroys;
    internal IntPtr LastOperation;
    internal string? Configuration;
    internal string? Intent;
    internal const string Activated = "{\"kind\":\"activated\",\"activation\":{\"source\":\"polar\",\"planIdentifier\":\"22222222-2222-4222-8222-222222222222\",\"activatedAt\":1700000000,\"licenseKey\":\"synthetic-key-not-sent\",\"activationIdentifier\":\"33333333-3333-4333-8333-333333333333\"}}";
    internal const string Validated = "{\"kind\":\"validated\",\"validation\":{\"isValid\":true,\"planIdentifier\":\"22222222-2222-4222-8222-222222222222\"}}";
    internal const string Cancelled = "{\"code\":\"cancelled\",\"mutationOutcome\":\"unknown\"}";

    private IntPtr Result(uint code, string json)
    {
        var result = checked((IntPtr)Interlocked.Increment(ref sequence));
        results[result] = (code, System.Text.Encoding.UTF8.GetBytes(json));
        return result;
    }
    public IntPtr Create(byte[] configuration, out IntPtr error)
    {
        Configuration = System.Text.Encoding.UTF8.GetString(configuration);
        error = InvalidConfiguration ? Result(1, "{\"code\":\"invalidProviderConfiguration\"}") : IntPtr.Zero;
        return InvalidConfiguration ? IntPtr.Zero : (IntPtr)1;
    }
    public void DestroyProvider(IntPtr provider) => Interlocked.Increment(ref ProviderDestroys);
    public IntPtr Start(IntPtr provider, byte[] intent, IntPtr context, SoftwareLicensingPolarCompletion completion)
    {
        Interlocked.Increment(ref Starts);
        if (ThrowOnStart) throw new InvalidOperationException("synthetic-private-diagnostic");
        if (ThrowCancellationOnStart) throw new OperationCanceledException("synthetic-private-diagnostic");
        Intent = System.Text.Encoding.UTF8.GetString(intent);
        if (NullStart) return IntPtr.Zero;
        var operation = checked((IntPtr)Interlocked.Increment(ref sequence));
        operations[operation] = new(context, completion);
        LastOperation = operation;
        OnStart?.Invoke();
        if (CompleteDuringStart) Complete(operation, 0, Activated);
        return operation;
    }
    internal void Complete(IntPtr operation, uint code, string json)
    {
        var entry = operations[operation];
        if (Interlocked.Exchange(ref entry.Completed, 1) != 0) throw new System.Diagnostics.UnreachableException("Duplicate fake completion.");
        entry.Completion(entry.Context, Result(code, json));
    }
    public void Cancel(IntPtr operation)
    {
        var entry = operations[operation];
        if (Volatile.Read(ref entry.Destroyed) != 0) throw new System.Diagnostics.UnreachableException("Cancel after native destruction.");
        Interlocked.Increment(ref Cancels);
        if (ThrowOnCancel) throw new InvalidOperationException("synthetic-private-diagnostic");
        if (CompleteDuringCancel && Volatile.Read(ref entry.Completed) == 0) Complete(operation, 1, Cancelled);
    }
    public void DestroyOperation(IntPtr operation)
    {
        var entry = operations[operation];
        if (Interlocked.Exchange(ref entry.Destroyed, 1) != 0) throw new System.Diagnostics.UnreachableException("Duplicate native destruction.");
        Interlocked.Increment(ref OperationDestroys);
        if (CompleteDuringDestroy && Volatile.Read(ref entry.Completed) == 0) Complete(operation, 1, Cancelled);
    }
    public JsonElement Consume(IntPtr result)
    {
        if (!results.TryRemove(result, out var value)) throw new System.Diagnostics.UnreachableException("Result missing or destroyed twice.");
        Interlocked.Increment(ref ResultDestroys);
        return Wire.Decode(value.Code, value.Bytes);
    }
}
