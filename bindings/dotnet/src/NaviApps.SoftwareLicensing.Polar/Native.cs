using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Win32.SafeHandles;

[assembly: InternalsVisibleTo("SoftwareLicensing.Polar.Tests")]

namespace NaviApps.SoftwareLicensing.Polar;

[UnmanagedFunctionPointer(CallingConvention.Cdecl)]
internal delegate void SoftwareLicensingPolarCompletion(IntPtr context, IntPtr result);

// Internal seam for deterministic ownership/race tests; production always uses the fixed Rust provider.
internal interface IProviderNative
{
    IntPtr Create(byte[] configuration, out IntPtr error);
    void DestroyProvider(IntPtr provider);
    IntPtr Start(IntPtr provider, byte[] intent, IntPtr context, SoftwareLicensingPolarCompletion completion);
    void Cancel(IntPtr operation);
    void DestroyOperation(IntPtr operation);
    JsonElement Consume(IntPtr result);
}

internal static class NativeFailure
{
    internal static LicenseException Cancelled(LicenseMutationOutcome outcome)
        => new(new LicenseFailure(LicenseErrorCode.Cancelled, MutationOutcome: outcome));

    internal static LicenseException Normalize(Exception error)
    {
        if (error is TypeInitializationException { InnerException: { } inner }) return Normalize(inner);
        return error switch
        {
            LicenseException failure => failure,
            OperationCanceledException => Cancelled(LicenseMutationOutcome.Unknown),
            DllNotFoundException or PlatformNotSupportedException => new(LicenseErrorCode.UnsupportedPlatform),
            EntryPointNotFoundException or BadImageFormatException => new(LicenseErrorCode.IncompatibleAbiVersion),
            _ => new(LicenseErrorCode.InvalidData)
        };
    }
}

internal sealed class ProviderHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private readonly IProviderNative native;
    internal ProviderHandle(IntPtr pointer, IProviderNative native) : base(true)
    {
        this.native = native;
        SetHandle(pointer);
    }
    protected override bool ReleaseHandle() { native.DestroyProvider(handle); return true; }
}

internal sealed class OperationHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private readonly IProviderNative native;
    internal OperationHandle(IntPtr pointer, IProviderNative native) : base(true)
    {
        this.native = native;
        SetHandle(pointer);
    }
    internal void Cancel()
    {
        bool held = false;
        try
        {
            DangerousAddRef(ref held);
            native.Cancel(DangerousGetHandle());
        }
        catch (ObjectDisposedException) { }
        finally { if (held) DangerousRelease(); }
    }
    protected override bool ReleaseHandle() { native.DestroyOperation(handle); return true; }
}

internal sealed class PInvokeProviderNative : IProviderNative
{
    private const string Library = "software_licensing_polar";
    internal static readonly PInvokeProviderNative Instance = new();
    static PInvokeProviderNative() => NativeLibrary.SetDllImportResolver(typeof(PInvokeProviderNative).Assembly, Resolve);
    private PInvokeProviderNative() { }
    private static IntPtr Resolve(string name, Assembly assembly, DllImportSearchPath? search)
    {
        if (name != Library) return IntPtr.Zero;
        var explicitPath = Environment.GetEnvironmentVariable("SOFTWARE_LICENSING_POLAR_NATIVE_LIBRARY");
        if (!string.IsNullOrEmpty(explicitPath)) return NativeLibrary.Load(explicitPath);
        string file = OperatingSystem.IsWindows() ? Library + ".dll" : OperatingSystem.IsMacOS() ? "lib" + Library + ".dylib" : "lib" + Library + ".so";
        string packaged = Path.Combine(AppContext.BaseDirectory, "runtimes", RuntimeInformation.RuntimeIdentifier, "native", file);
        return File.Exists(packaged) ? NativeLibrary.Load(packaged) : NativeLibrary.Load(file, assembly, search);
    }
    [StructLayout(LayoutKind.Sequential)]
    private readonly struct SoftwareLicensingPolarResultView { public readonly uint Code; public readonly IntPtr Data; public readonly nuint Length; }
    [DllImport(Library, EntryPoint = "software_licensing_polar_abi_version", CallingConvention = CallingConvention.Cdecl)]
    private static extern uint AbiVersion();
    [DllImport(Library, EntryPoint = "software_licensing_polar_provider_create", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr ProviderCreate(byte[] data, nuint length, out IntPtr error);
    [DllImport(Library, EntryPoint = "software_licensing_polar_provider_destroy", CallingConvention = CallingConvention.Cdecl)]
    private static extern void ProviderDestroy(IntPtr provider);
    [DllImport(Library, EntryPoint = "software_licensing_polar_operation_start", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr OperationStart(IntPtr provider, byte[] data, nuint length, IntPtr context, SoftwareLicensingPolarCompletion completion);
    [DllImport(Library, EntryPoint = "software_licensing_polar_operation_cancel", CallingConvention = CallingConvention.Cdecl)]
    private static extern void OperationCancel(IntPtr operation);
    [DllImport(Library, EntryPoint = "software_licensing_polar_operation_destroy", CallingConvention = CallingConvention.Cdecl)]
    private static extern void OperationDestroy(IntPtr operation);
    [DllImport(Library, EntryPoint = "software_licensing_polar_result_view", CallingConvention = CallingConvention.Cdecl)]
    private static extern SoftwareLicensingPolarResultView ResultView(IntPtr result);
    [DllImport(Library, EntryPoint = "software_licensing_polar_result_destroy", CallingConvention = CallingConvention.Cdecl)]
    private static extern void ResultDestroy(IntPtr result);

    public IntPtr Create(byte[] configuration, out IntPtr error)
    {
        if (AbiVersion() != 3) throw new LicenseException(LicenseErrorCode.IncompatibleAbiVersion);
        return ProviderCreate(configuration, (nuint)configuration.Length, out error);
    }
    public void DestroyProvider(IntPtr provider) => ProviderDestroy(provider);
    public IntPtr Start(IntPtr provider, byte[] intent, IntPtr context, SoftwareLicensingPolarCompletion completion)
        => OperationStart(provider, intent, (nuint)intent.Length, context, completion);
    public void Cancel(IntPtr operation) => OperationCancel(operation);
    public void DestroyOperation(IntPtr operation) => OperationDestroy(operation);
    public JsonElement Consume(IntPtr result)
    {
        if (result == IntPtr.Zero) throw new LicenseException(LicenseErrorCode.InvalidData);
        try
        {
            var view = ResultView(result);
            if (view.Data == IntPtr.Zero || view.Length > 1024 * 1024)
                throw new LicenseException(LicenseErrorCode.InvalidData);
            var bytes = new byte[checked((int)view.Length)];
            try
            {
                Marshal.Copy(view.Data, bytes, 0, bytes.Length);
                return Wire.Decode(view.Code, bytes);
            }
            finally { System.Security.Cryptography.CryptographicOperations.ZeroMemory(bytes); }
        }
        finally { ResultDestroy(result); }
    }
}
