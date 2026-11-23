namespace NaviApps.SoftwareLicensing.Polar;

/// <summary>The fixed Polar API environment used for every operation. The default value is invalid.</summary>
public enum PolarLicenseEnvironment
{
    Sandbox = 1,
    Production = 2,
}

/// <summary>Required organization and nonempty allowed license-key benefit identifiers.</summary>
public sealed record PolarLicenseScope(string OrganizationId, IReadOnlyList<string> BenefitIds);

/// <summary>Optional retry and timeout values override the defaults of the native provider.</summary>
public sealed record PolarLicenseConfiguration(
    string ActivationInstanceName,
    PolarLicenseScope LicenseScope,
    PolarLicenseEnvironment Environment,
    int? MaximumRequestAttempts = null,
    double? BaseRetryDelay = null,
    double? MaximumRetryDelay = null,
    double? RequestTimeoutSeconds = null,
    double? OperationTimeoutSeconds = null);
