# ``SoftwareLicensingPolar``

Use Polar licensing in macOS 26 or later apps with Swift 6.3 or later.

## Overview

`PolarLicenseProvider` supplies activation, validation and deactivation to
SoftwareLicensing. Rust owns HTTP, Polar API versioning, response validation,
deadlines and retries. Swift bridges values, completion and cancellation.
Constructing a provider validates configuration without sending a request.

Choose an explicit sandbox or production environment, your organization UUID and
a nonempty allowlist of license-key benefit UUIDs. The SDK accepts only keys and
activations matching that scope. It does not send merchant credentials, test a
key against both environments, or migrate existing provider records.

This catalog describes the Swift module. The same Rust implementation also backs
the .NET package; SwiftPM ships a macOS universal framework. Windows execution and
real provider acceptance require separate verification.

The host owns secure-storage namespaces, UI, scheduling and commercial policy.
Preserve the stored source `polar` and the original activation identity.
`planIdentifier` is the accepted Polar benefit UUID.
An explicit null expiration occurs for subscriptions too; it never establishes
that a plan is lifetime. Configure benefit-to-plan policy in the host.

Create the core manager with `try LicenseManager(...)`. Native ABI incompatibility
is `LicenseError.Code.incompatibleABIVersion`; `unsupportedVersion` remains a
persisted-record version error. Keep matching core bindings and native artifacts.
The core's typed `LicenseRefreshOutcome` describes refresh/release decisions;
validation failures may retain bounded grace and remain visible in the state.

## Topics

### Provider

- ``PolarLicenseProvider``
- ``PolarLicenseProvider/Configuration``
- ``PolarLicenseProvider/LicenseScope``
- ``PolarLicenseProvider/Environment``

## Failure and recovery

Mutation failures distinguish `notApplied` from `unknown`. Cancellation preserves
that outcome and cannot undo a server-side activation. Use the core manager for
durable activation intent, saved refresh eligibility and explicit recovery.
The provider stops the operation on an ambiguous mutation. A generic 404 is not proof
of license revocation or successful deactivation. A 4xx without a single matching
`Polar-Version` leaves a mutation outcome unknown. Rate-limit failures retain
server retry timing across operation completion. Only a 429 with the matching
version can authorize retry of a mutation within that operation.
