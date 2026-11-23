import Foundation
import SoftwareLicensing
import XCTest

@testable import SoftwareLicensingPolar

final class ProviderTests: XCTestCase {
  private let organizationID = "11111111-1111-4111-8111-111111111111"
  private let benefitID = "22222222-2222-4222-8222-222222222222"

  private var scope: PolarLicenseProvider.LicenseScope {
    .init(organizationID: organizationID, benefitIDs: [benefitID])
  }

  private func makeProvider() throws -> PolarLicenseProvider {
    try PolarLicenseProvider(
      configuration: .init(
        activationInstanceName: "Fixture", licenseScope: scope, environment: .sandbox))
  }

  func testNativeInvalidScopeReturnsTypedConfigurationError() {
    XCTAssertThrowsError(
      try PolarLicenseProvider(
        configuration: .init(
          activationInstanceName: "Fixture",
          licenseScope: .init(organizationID: organizationID, benefitIDs: []), environment: .sandbox
        ))
    ) {
      XCTAssertEqual(($0 as? LicenseError)?.code, .invalidProviderConfiguration)
    }
  }

  func testBothExplicitEnvironmentsConstructWithoutRequests() {
    for environment in [PolarLicenseProvider.Environment.production, .sandbox] {
      XCTAssertNoThrow(
        try PolarLicenseProvider(
          configuration: .init(
            activationInstanceName: "Fixture", licenseScope: scope, environment: environment)))
    }
  }

  func testConfigurationEncodesPolarScopeAndExplicitEnvironment() throws {
    let configuration = PolarLicenseProvider.Configuration(
      activationInstanceName: "Fixture", licenseScope: scope, environment: .sandbox)
    let encoded = try JSONEncoder().encode(configuration)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    let licenseScope = try XCTUnwrap(object["licenseScope"] as? [String: Any])
    XCTAssertEqual(object["environment"] as? String, "sandbox")
    XCTAssertEqual(licenseScope["organizationId"] as? String, organizationID)
    XCTAssertEqual(licenseScope["benefitIds"] as? [String], [benefitID])
    XCTAssertNil(licenseScope["productId"])
    for key in [
      "maximumRequestAttempts", "baseRetryDelay", "maximumRetryDelay",
      "requestTimeoutSeconds", "operationTimeoutSeconds",
    ] {
      XCTAssertNil(object[key], "Rust supplies the default for \(key)")
    }
    XCTAssertEqual(
      try JSONDecoder().decode(PolarLicenseProvider.Configuration.self, from: encoded),
      configuration)
  }

  func testExplicitRetryAndTimeoutOverridesRoundTrip() throws {
    let configuration = PolarLicenseProvider.Configuration(
      activationInstanceName: "Fixture", licenseScope: scope, environment: .sandbox,
      maximumRequestAttempts: 2, baseRetryDelay: 0.5, maximumRetryDelay: 20,
      requestTimeoutSeconds: 45, operationTimeoutSeconds: 90)
    let encoded = try JSONEncoder().encode(configuration)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    XCTAssertEqual(object["maximumRequestAttempts"] as? Int, 2)
    XCTAssertEqual(object["baseRetryDelay"] as? Double, 0.5)
    XCTAssertEqual(object["maximumRetryDelay"] as? Double, 20)
    XCTAssertEqual(object["requestTimeoutSeconds"] as? Double, 45)
    XCTAssertEqual(object["operationTimeoutSeconds"] as? Double, 90)
    XCTAssertEqual(
      try JSONDecoder().decode(PolarLicenseProvider.Configuration.self, from: encoded),
      configuration)
    XCTAssertNoThrow(try PolarLicenseProvider(configuration: configuration))
  }

  func testValidationProjectsTypedForeignProviderRejection() async throws {
    let provider = try makeProvider()
    let activation = try XCTUnwrap(
      LicenseActivation(
        source: LicenseSource(identifier: "fixture-provider")!,
        planIdentifier: benefitID, activatedAt: Date(timeIntervalSince1970: 1000),
        licenseKey: "fixture-key", activationIdentifier: "33333333-3333-4333-8333-333333333333"))
    do {
      _ = try await provider.validate(activation)
      XCTFail("Polar must reject activation from another provider during validation")
    } catch let error as LicenseError {
      XCTAssertEqual(error.code, .invalidLicense)
      XCTAssertNil(error.mutationOutcome)
    }
  }

  func testAttemptCountsOutsideNativeUInt32ReturnConfigurationError() {
    for attempts in [-1, Int.max] {
      XCTAssertThrowsError(
        try PolarLicenseProvider(
          configuration: .init(
            activationInstanceName: "Fixture", licenseScope: scope, environment: .sandbox,
            maximumRequestAttempts: attempts))
      ) {
        XCTAssertEqual(($0 as? LicenseError)?.code, .invalidProviderConfiguration)
      }
    }
  }

  func testNonfiniteConfigurationIsRejectedWithoutNetwork() {
    XCTAssertThrowsError(
      try PolarLicenseProvider(
        configuration: .init(
          activationInstanceName: "Fixture", licenseScope: scope, environment: .sandbox,
          operationTimeoutSeconds: .infinity))
    ) {
      XCTAssertEqual(($0 as? LicenseError)?.code, .invalidProviderConfiguration)
    }
  }

  func testAutomaticActivationIsRejectedByTheLicenseAPIProvider() async throws {
    let provider = try makeProvider()
    do {
      _ = try await provider.activate(.automatic)
      XCTFail("Key-based licensing cannot automatically activate")
    } catch let error as LicenseError {
      XCTAssertEqual(error.code, .invalidLicenseKey)
    }
  }

  func testPrecancelledTaskReportsTypedNotAppliedWithoutStartingARequest() async throws {
    let provider = try makeProvider()
    let operation = Task {
      withUnsafeCurrentTask { $0?.cancel() }
      return try await provider.activate(.licenseKey("must-not-be-sent"))
    }
    do {
      _ = try await operation.value
      XCTFail("Cancellation must be preserved with its mutation outcome")
    } catch let error as LicenseError {
      XCTAssertEqual(error.code, .cancelled)
      XCTAssertEqual(error.mutationOutcome, .notApplied)
    }
  }

  func testCancellationBeforePreparationReportsTypedNotApplied() async throws {
    let call = NativeCall()
    call.cancel()
    do {
      let _: Data = try await withCheckedThrowingContinuation { continuation in
        let prepared = call.prepare(continuation)
        XCTAssertFalse(prepared)
        if prepared { call.complete(.success(Data())) }
      }
      XCTFail("A cancelled call must not be prepared for native start")
    } catch let error as LicenseError {
      XCTAssertEqual(error.code, .cancelled)
      XCTAssertEqual(error.mutationOutcome, .notApplied)
    }
  }

  func testNativeFailureMetadataSurvivesHostCancellation() async throws {
    for (code, outcome) in [
      (LicenseError.Code.cancelled, LicenseMutationOutcome.notApplied),
      (.cancelled, .unknown), (.rateLimited, .notApplied),
    ] {
      let data = Data(
        """
        {"code":"\(code.rawValue)","statusCode":429,"retryAfterSeconds":18446744073709551615,"mutationOutcome":"\(outcome.rawValue)"}
        """.utf8)
      let call = NativeCall()
      do {
        let _: Data = try await withCheckedThrowingContinuation { continuation in
          XCTAssertTrue(call.prepare(continuation))
          call.cancel()
          call.complete(
            Result { try PolarLicenseProvider.decodeResult(data, code: 1) })
        }
        XCTFail("Native failures must retain their typed evidence")
      } catch let error as LicenseError {
        XCTAssertEqual(error.code, code)
        XCTAssertEqual(error.statusCode, 429)
        XCTAssertEqual(error.retryAfterSeconds, UInt64.max)
        XCTAssertEqual(error.mutationOutcome, outcome)
      }
    }
  }

  func testSerializedNativeFailuresRetainRetryMetadataWithoutInventingAnOutcome() {
    let limited = Data(
      #"{"code":"rateLimited","statusCode":429,"retryAfterSeconds":18446744073709551615,"mutationOutcome":"notApplied"}"#
        .utf8)
    XCTAssertThrowsError(try PolarLicenseProvider.decodeResult(limited, code: 1)) {
      let error = $0 as? LicenseError
      XCTAssertEqual(error?.code, .rateLimited)
      XCTAssertEqual(error?.statusCode, 429)
      XCTAssertEqual(error?.retryAfterSeconds, UInt64.max)
      XCTAssertEqual(error?.mutationOutcome, .notApplied)
    }
    let cancelled = Data(#"{"code":"cancelled"}"#.utf8)
    XCTAssertThrowsError(try PolarLicenseProvider.decodeResult(cancelled, code: 1)) {
      let error = $0 as? LicenseError
      XCTAssertEqual(error?.code, .cancelled)
      XCTAssertNil(error?.mutationOutcome)
    }
  }

  func testConcurrentEarlyCompletionsKeepOwnersIndependent() async throws {
    let provider = try makeProvider()
    try await withThrowingTaskGroup(of: Void.self) { group in
      for _ in 0..<100 {
        group.addTask {
          do {
            _ = try await provider.activate(.licenseKey(""))
            throw LicenseError(.unexpectedProviderResponse)
          } catch let failure as LicenseError {
            XCTAssertEqual(failure.code, .invalidLicenseKey)
          }
        }
      }
      try await group.waitForAll()
    }
  }

  func testCancellationDoesNotDiscardAConfirmedProviderReply() async throws {
    let call = NativeCall()
    let confirmed = Data(#"{"kind":"deactivated"}"#.utf8)
    let value: Data = try await withCheckedThrowingContinuation { continuation in
      XCTAssertTrue(call.prepare(continuation))
      call.cancel()
      call.complete(.success(confirmed))
    }
    XCTAssertEqual(value, confirmed)
  }
}
