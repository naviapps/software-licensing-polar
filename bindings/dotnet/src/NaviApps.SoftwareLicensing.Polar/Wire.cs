using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.Json.Serialization.Metadata;

namespace NaviApps.SoftwareLicensing.Polar;

internal sealed record PolarIntent(string Operation, string? LicenseKey = null, LicenseActivation? Activation = null);

/// <summary>Operation-boundary DTO framing only. HTTP and provider response codecs live in Rust.</summary>
internal static class Wire
{
    private static readonly WireJsonContext Context = new(new JsonSerializerOptions
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        Converters =
        {
            new JsonStringEnumConverter<PolarLicenseEnvironment>(JsonNamingPolicy.CamelCase, allowIntegerValues: false),
            new JsonStringEnumConverter<LicenseErrorCode>(JsonNamingPolicy.CamelCase, allowIntegerValues: false),
            new JsonStringEnumConverter<LicenseMutationOutcome>(JsonNamingPolicy.CamelCase, allowIntegerValues: false),
            new JsonStringEnumConverter<LicenseStatus>(JsonNamingPolicy.CamelCase, allowIntegerValues: false),
            new UnixDateConverter(),
            new ExactStringConverter(),
        }
    });

    internal static JsonTypeInfo<T> Type<T>() => (JsonTypeInfo<T>)Context.GetTypeInfo(typeof(T))!;

    private sealed class ExactStringConverter : JsonConverter<string>
    {
        private static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

        public override string? Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options) =>
            reader.GetString();

        public override void Write(Utf8JsonWriter writer, string value, JsonSerializerOptions options)
        {
            // JSON would otherwise replace malformed UTF-16 in an identity or key before Rust sees it.
            try { _ = StrictUtf8.GetByteCount(value); }
            catch (EncoderFallbackException) { throw new JsonException("Expected valid Unicode text."); }
            writer.WriteStringValue(value);
        }
    }

    private sealed class UnixDateConverter : JsonConverter<DateTimeOffset>
    {
        public override DateTimeOffset Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options)
            => DateTimeOffset.UnixEpoch.AddSeconds(reader.GetDouble());
        public override void Write(Utf8JsonWriter writer, DateTimeOffset value, JsonSerializerOptions options)
            => writer.WriteNumberValue((value - DateTimeOffset.UnixEpoch).TotalSeconds);
    }
    internal static JsonElement Decode(uint code, byte[] bytes)
    {
        try
        {
            using var document = JsonDocument.Parse(bytes);
            if (code == 1) throw new LicenseException(document.RootElement.Deserialize(Type<LicenseFailure>())
                ?? new(LicenseErrorCode.UnexpectedProviderResponse));
            if (code != 0) throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse);
            return document.RootElement.Clone();
        }
        catch (JsonException) { throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse); }
    }
    internal static T Reply<T>(JsonElement reply, string kind, string property)
    {
        try
        {
            RequireKind(reply, kind);
            return reply.GetProperty(property).Deserialize(Type<T>())
                ?? throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse);
        }
        catch (Exception error) when (error is JsonException or InvalidOperationException or KeyNotFoundException or ArgumentException)
        { throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse); }
    }
    internal static void RequireKind(JsonElement reply, string kind)
    {
        try
        {
            if (reply.GetProperty("kind").GetString() != kind) throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse);
        }
        catch (Exception error) when (error is InvalidOperationException or KeyNotFoundException)
        { throw new LicenseException(LicenseErrorCode.UnexpectedProviderResponse); }
    }
}

[JsonSourceGenerationOptions(GenerationMode = JsonSourceGenerationMode.Metadata)]
[JsonSerializable(typeof(PolarLicenseConfiguration))]
[JsonSerializable(typeof(PolarIntent))]
[JsonSerializable(typeof(LicenseActivation))]
[JsonSerializable(typeof(LicenseValidationResult))]
[JsonSerializable(typeof(LicenseFailure))]
internal sealed partial class WireJsonContext : JsonSerializerContext { }
