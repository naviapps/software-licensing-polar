import Foundation
import SoftwareLicensing
import SoftwareLicensingPolar
import Synchronization

private struct AcceptanceFailure: Error {
  let message: String
}

private func require(_ condition: Bool, _ message: String) throws {
  guard condition else { throw AcceptanceFailure(message: message) }
}

private final class InMemoryLicenseStorage: LicenseStorage {
  private struct State: Sendable {
    var record: Data?
    var saveCount = 0
    var failOnSave: Int?
  }

  private let state: Mutex<State>

  init(record: Data? = nil) { state = Mutex(State(record: record)) }
  func load() throws -> Data? { state.withLock { $0.record } }
  var saveCount: Int { state.withLock { $0.saveCount } }
  func failSave(_ attempt: Int) { state.withLock { $0.failOnSave = attempt } }

  func save(_ record: Data) throws {
    try state.withLock { value in
      value.saveCount += 1
      if value.failOnSave == value.saveCount { throw LicenseError(.storageFailure) }
      value.record = record
    }
  }
}

@MainActor
private func recoverPolarPreflight(_ provider: PolarLicenseProvider) async throws {
  let clock: @Sendable () -> Date = { Date(timeIntervalSince1970: 2000) }
  let storage = InMemoryLicenseStorage()
  let manager = try LicenseManager(provider: provider, storage: storage, clock: clock)
  let cancelled = Task { @MainActor in
    withUnsafeCurrentTask { $0?.cancel() }
    return try await manager.activate(.automatic)
  }
  do {
    _ = try await cancelled.value
    throw AcceptanceFailure(message: "A cancelled activation was allowed to proceed")
  } catch is CancellationError {
    try require(storage.saveCount == 0, "A cancelled activation touched storage")
  }
  // The keyless request reaches Polar's native preflight after the first save.
  storage.failSave(2)
  do {
    try await manager.activate(.automatic)
    throw AcceptanceFailure(message: "The failed save went unreported")
  } catch let error as LicenseError {
    try require(error.code == .storageFailure, "The failed save lost its typed error")
  }
  try require(
    manager.state.hasPendingPersistence && !manager.state.isLicensed && storage.saveCount == 2,
    "The failed save did not retain the inaccessible pending operation")
  let rejection = LicenseError(.invalidLicenseKey, mutationOutcome: .notApplied)
  do {
    try await manager.retryPersistence()
    throw AcceptanceFailure(message: "The retry lost the original Polar rejection")
  } catch let error as LicenseError {
    try require(error == rejection, "The retry lost the Polar mutation evidence")
  }
  try require(storage.saveCount == 3, "Persistence retry performed an unexpected write")
  let reopenedStorage = InMemoryLicenseStorage(record: try storage.load())
  let reopened = try LicenseManager(provider: provider, storage: reopenedStorage, clock: clock)
  let snapshot = reopened.state
  try require(
    reopened.initialRestoreError == nil && !snapshot.hasPendingActivation
      && !snapshot.hasPendingPersistence && !snapshot.hasActivation
      && snapshot.lastOperationFailure == rejection,
    "Reopening lost the typed rejection or left a pending activation")
  let refresh = try await reopened.refresh(onlyIfDue: true)
  try require(
    refresh.outcome == .skippedNoActivation && reopenedStorage.saveCount == 0,
    "Startup refresh acted on a rejected activation")
}

@main
struct PolarConsumer {
  @MainActor
  static func main() async throws {
    do {
      _ = try PolarLicenseProvider(
        configuration: .init(
          activationInstanceName: "Independent consumer",
          licenseScope: .init(organizationID: "", benefitIDs: []),
          environment: .sandbox))
      throw AcceptanceFailure(message: "An empty Polar scope was accepted")
    } catch let error as LicenseError {
      try require(
        error.code == .invalidProviderConfiguration,
        "The constructor did not preserve the public configuration error")
    }

    let provider = try PolarLicenseProvider(
      configuration: .init(
        activationInstanceName: "Independent consumer",
        licenseScope: .init(
          organizationID: "11111111-1111-4111-8111-111111111111",
          benefitIDs: ["22222222-2222-4222-8222-222222222222"]), environment: .sandbox,
        requestTimeoutSeconds: 45))
    // Both requests have no usable license key and fail before an HTTP request is built.
    for request in [LicenseActivationRequest.automatic, .licenseKey("")] {
      do {
        _ = try await provider.activate(request)
        throw AcceptanceFailure(message: "A keyless activation was accepted")
      } catch let error as LicenseError {
        try require(
          error == LicenseError(.invalidLicenseKey, mutationOutcome: .notApplied),
          "The native completion did not preserve the public invalid-key error")
      }
    }
    try await recoverPolarPreflight(provider)
    print("Standalone SoftwareLicensingPolar acceptance passed.")
  }
}
