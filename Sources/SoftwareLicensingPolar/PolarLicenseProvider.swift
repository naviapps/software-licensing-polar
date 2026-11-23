import CSoftwareLicensingPolar
import Foundation
import SoftwareLicensing

/// License API effects are executed entirely by Rust. No merchant API is used.
public final class PolarLicenseProvider: LicenseProvider, @unchecked Sendable {
  /// The fixed Polar API deployment to use. Choose explicitly; keys are not probed across environments.
  public enum Environment: String, Codable, Equatable, Sendable {
    case production
    case sandbox
  }

  public struct LicenseScope: Codable, Equatable, Sendable {
    public let organizationID: String
    public let benefitIDs: [String]

    /// IDs are canonical UUIDs. A nonempty benefit allowlist prevents accepting another product's key.
    public init(organizationID: String, benefitIDs: [String]) {
      self.organizationID = organizationID
      self.benefitIDs = benefitIDs
    }

    private enum CodingKeys: String, CodingKey {
      case organizationID = "organizationId"
      case benefitIDs = "benefitIds"
    }
  }

  public struct Configuration: Codable, Equatable, Sendable {
    public let activationInstanceName: String
    public let licenseScope: LicenseScope
    public let environment: Environment
    public let maximumRequestAttempts: Int?
    public let baseRetryDelay: TimeInterval?
    public let maximumRetryDelay: TimeInterval?
    public let requestTimeoutSeconds: TimeInterval?
    public let operationTimeoutSeconds: TimeInterval?

    /// Omitted retry and timeout options use the defaults of the native provider.
    public init(
      activationInstanceName: String, licenseScope: LicenseScope, environment: Environment,
      maximumRequestAttempts: Int? = nil,
      baseRetryDelay: TimeInterval? = nil, maximumRetryDelay: TimeInterval? = nil,
      requestTimeoutSeconds: TimeInterval? = nil, operationTimeoutSeconds: TimeInterval? = nil
    ) {
      self.activationInstanceName = activationInstanceName
      self.licenseScope = licenseScope
      self.environment = environment
      self.maximumRequestAttempts = maximumRequestAttempts
      self.baseRetryDelay = baseRetryDelay
      self.maximumRetryDelay = maximumRetryDelay
      self.requestTimeoutSeconds = requestTimeoutSeconds
      self.operationTimeoutSeconds = operationTimeoutSeconds
    }
  }

  // Immutable Rust owner; each started operation clones its own Arc before returning.
  private let handle: OpaquePointer

  /// Validates configuration without contacting the provider.
  public init(configuration: Configuration) throws {
    guard software_licensing_polar_abi_version() == 3 else {
      throw LicenseError(.incompatibleABIVersion)
    }
    let data: Data
    do { data = try JSONEncoder().encode(configuration) } catch {
      throw LicenseError(.invalidProviderConfiguration)
    }
    var error: OpaquePointer?
    let owner = data.withUnsafeBytes { bytes in
      software_licensing_polar_provider_create(
        bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, &error)
    }
    guard let owner else {
      if let error { _ = try Self.consume(error) }
      throw LicenseError(.invalidProviderConfiguration)
    }
    handle = owner
  }

  deinit { software_licensing_polar_provider_destroy(handle) }

  public func activate(_ request: LicenseActivationRequest) async throws -> LicenseActivation {
    let key: String? = if case .licenseKey(let key) = request { key } else { nil }
    let reply = try await perform(Intent(operation: "activate", licenseKey: key))
    guard reply.kind == .activated, let activation = reply.activation else {
      throw LicenseError(.unexpectedProviderResponse)
    }
    return activation
  }

  public func validate(_ activation: LicenseActivation) async throws -> LicenseValidationResult {
    let reply = try await perform(Intent(operation: "validate", activation: activation))
    guard reply.kind == .validated, let validation = reply.validation else {
      throw LicenseError(.unexpectedProviderResponse)
    }
    return validation
  }

  public func deactivate(_ activation: LicenseActivation) async throws {
    guard
      try await perform(Intent(operation: "deactivate", activation: activation)).kind
        == .deactivated
    else {
      throw LicenseError(.unexpectedProviderResponse)
    }
  }

  private struct Intent: Encodable {
    let operation: String
    var licenseKey: String?
    var activation: LicenseActivation?
  }

  private struct Reply: Decodable {
    enum Kind: String, Decodable { case activated, validated, deactivated }
    let kind: Kind
    let activation: LicenseActivation?
    let validation: LicenseValidationResult?
  }

  private func perform(_ intent: Intent) async throws -> Reply {
    if Task.isCancelled { throw LicenseError(.cancelled, mutationOutcome: .notApplied) }
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .secondsSince1970
    let data: Data
    do { data = try encoder.encode(intent) } catch { throw LicenseError(.invalidData) }
    let call = NativeCall()
    let result: Data = try await withTaskCancellationHandler {
      if Task.isCancelled { throw LicenseError(.cancelled, mutationOutcome: .notApplied) }
      return try await withCheckedThrowingContinuation { continuation in
        guard call.prepare(continuation) else { return }
        let context = Unmanaged.passRetained(call).toOpaque()
        let operation = data.withUnsafeBytes { bytes in
          software_licensing_polar_operation_start(
            handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, context,
            { context, result in
              guard let context else {
                software_licensing_polar_result_destroy(result)
                return
              }
              let call = Unmanaged<NativeCall>.fromOpaque(context).takeRetainedValue()
              call.complete(Result { try PolarLicenseProvider.consume(result) })
            })
        }
        if let operation {
          call.install(operation)
        } else {
          Unmanaged<NativeCall>.fromOpaque(context).release()
          call.complete(.failure(LicenseError(.invalidData)))
        }
      }
    } onCancel: {
      call.cancel()
    }
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .secondsSince1970
    do { return try decoder.decode(Reply.self, from: result) } catch {
      throw LicenseError(.unexpectedProviderResponse)
    }
  }

  private static func consume(_ result: OpaquePointer?) throws -> Data {
    guard let result else { throw LicenseError(.invalidData) }
    defer { software_licensing_polar_result_destroy(result) }
    let view = software_licensing_polar_result_view(result)
    guard let bytes = view.data, view.length <= 1_048_576 else { throw LicenseError(.invalidData) }
    let data = Data(bytes: bytes, count: view.length)
    return try decodeResult(data, code: view.code)
  }

  static func decodeResult(_ data: Data, code: UInt32) throws -> Data {
    if code != 0 {
      // The core manager needs typed cancellation metadata to reconcile durable mutations.
      throw (try? JSONDecoder().decode(LicenseError.self, from: data)) ?? LicenseError(.invalidData)
    }
    return data
  }
}

// Cancellation never borrows a pointer outside this lock. Completion releases its
// owner after removing it; early callbacks are safe before start installs the handle.
final class NativeCall: @unchecked Sendable {
  private let lock = NSLock()
  private var continuation: CheckedContinuation<Data, any Error>?
  private var operation: OpaquePointer?
  private var cancelled = false
  private var completed = false

  func prepare(_ continuation: CheckedContinuation<Data, any Error>) -> Bool {
    lock.lock()
    if cancelled {
      completed = true
      lock.unlock()
      continuation.resume(throwing: LicenseError(.cancelled, mutationOutcome: .notApplied))
      return false
    }
    self.continuation = continuation
    lock.unlock()
    return true
  }

  func install(_ operation: OpaquePointer) {
    lock.lock()
    if completed {
      lock.unlock()
      software_licensing_polar_operation_destroy(operation)
      return
    }
    self.operation = operation
    if cancelled { software_licensing_polar_operation_cancel(operation) }
    lock.unlock()
  }

  func cancel() {
    lock.lock()
    cancelled = true
    if let operation { software_licensing_polar_operation_cancel(operation) }
    lock.unlock()
  }

  func complete(_ result: Result<Data, any Error>) {
    lock.lock()
    guard !completed else {
      lock.unlock()
      return
    }
    completed = true
    let continuation = self.continuation
    self.continuation = nil
    let operation = self.operation
    self.operation = nil
    lock.unlock()
    if let operation { software_licensing_polar_operation_destroy(operation) }
    // Preserve a confirmed server result even when host cancellation races with completion.
    // The core manager must be able to persist an issued activation or released instance.
    continuation?.resume(with: result)
  }
}
