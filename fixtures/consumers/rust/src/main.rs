use software_licensing::{Activation, ErrorCode, MutationOutcome};
use software_licensing_polar::{
    Environment, LicenseScope, PolarLicenseConfig, PolarLicenseProvider,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let provider = PolarLicenseProvider::new(PolarLicenseConfig {
        activation_instance_name: "Fixture".into(),
        license_scope: LicenseScope {
            organization_id: "11111111-1111-4111-8111-111111111111".into(),
            benefit_ids: vec!["22222222-2222-4222-8222-222222222222".into()],
        },
        environment: Environment::Sandbox,
        maximum_request_attempts: 1,
        base_retry_delay: 0.1,
        maximum_retry_delay: 0.1,
        request_timeout_seconds: 1.0,
        operation_timeout_seconds: 1.0,
    })
    .expect("Build a provider without sending a request");

    // These values deliberately cannot produce HTTP requests.
    let invalid_source = Activation {
        source: "fixture".into(),
        plan_identifier: "paid".into(),
        activated_at: 1000.0,
        license_key: None,
        activation_identifier: None,
        expires_at: None,
    };
    let validation_failure = provider.validate(invalid_source).await.unwrap_err();
    assert_eq!(validation_failure.code, ErrorCode::InvalidLicense);
    assert_eq!(validation_failure.mutation_outcome, None);

    let activation_failure = provider.activate("").await.unwrap_err();
    assert_eq!(activation_failure.code, ErrorCode::InvalidLicenseKey);
    assert_eq!(
        activation_failure.mutation_outcome,
        Some(MutationOutcome::NotApplied)
    );
    println!(
        "Standalone Rust Git Polar consumer passed typed core and provider preflight."
    );
}
