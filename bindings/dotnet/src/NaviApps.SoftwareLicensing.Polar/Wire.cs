using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace NaviApps.SoftwareLicensing.Polar;

/// <summary>Operation-boundary DTO framing only. HTTP and provider response codecs live in Rust.</summary>
internal static class Wire
{
    internal static readonly JsonSerializerOptions Json = Options();
    private static JsonSerializerOptions Options()
    {
        var options = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
        };
        options.Converters.Add(new JsonStringEnumConverter(JsonNamingPolicy.CamelCase, allowIntegerValues: false));
        options.Converters.Add(new UnixDateConverter());
        options.Converters.Add(new ExactStringConverter());
        return options;
    }
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
            if (code == 1) throw new LicenseException(document.RootElement.Deserialize<LicenseFailure>(Json)
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
            return reply.GetProperty(property).Deserialize<T>(Json)
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
