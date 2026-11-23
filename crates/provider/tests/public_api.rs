use serde_json::{Value, json};
use software_licensing::{Activation, ErrorCode, Failure, ProviderReply, Validation};
use software_licensing_polar::{Intent, LicenseScope, PolarLicenseConfig, PolarLicenseProvider};

fn config_wire() -> serde_json::Value {
    json!({
        "environment": "sandbox",
        "activationInstanceName": "Fixture",
        "licenseScope": {"organizationId": "11111111-1111-4111-8111-111111111111", "benefitIds": ["22222222-2222-4222-8222-222222222222"]}
    })
}

fn config() -> PolarLicenseConfig {
    serde_json::from_value(config_wire()).unwrap()
}

#[test]
fn public_config_defaults_and_construction_need_no_runtime_or_network() {
    let config = config();
    let _: &LicenseScope = &config.license_scope;
    assert_eq!(config.request_timeout_seconds, 30.0);
    assert_eq!(config.operation_timeout_seconds, 60.0);
    assert_eq!(config.maximum_request_attempts, 3);
    assert_eq!(config.base_retry_delay, 0.2);
    assert_eq!(config.maximum_retry_delay, 30.0);
    let provider = PolarLicenseProvider::new(config).unwrap();
    fn send_sync<T: Send + Sync>(_: &T) {}
    send_sync(&provider);
    let intent: Intent = serde_json::from_value(json!({
        "operation": "activate", "licenseKey": "FIXTURE-KEY"
    }))
    .unwrap();
    fn send_future<T>(_: impl Future<Output = Result<T, Failure>> + Send) {}
    // Constructing and dropping an unpolled execution does no work.
    send_future::<ProviderReply>(provider.execute(intent));
    let intent: Intent = serde_json::from_value(json!({
        "operation": "activate", "licenseKey": "FIXTURE-KEY"
    }))
    .unwrap();
    send_future::<ProviderReply>(
        provider.execute_with_cancellation(intent, std::future::pending()),
    );
    send_future::<Activation>(provider.activate("FIXTURE-KEY"));
    let activation: Activation = serde_json::from_value(json!({
        "source": "polar", "planIdentifier": "22222222-2222-4222-8222-222222222222", "activatedAt": 1700000000,
        "licenseKey": "FIXTURE-KEY", "activationIdentifier": "44444444-4444-4444-8444-444444444444"
    }))
    .unwrap();
    send_future::<Validation>(provider.validate(activation.clone()));
    send_future::<()>(provider.deactivate(activation));
}

#[test]
fn config_wire_requires_explicit_environment_and_scope() {
    let valid = config_wire();
    for field in ["environment", "licenseScope", "activationInstanceName"] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<PolarLicenseConfig>(value).is_err(),
            "{field}"
        );
    }
    for environment in [
        Value::Null,
        json!(0),
        json!(1),
        json!(2),
        json!(""),
        json!("staging"),
    ] {
        let mut value = valid.clone();
        value["environment"] = environment;
        assert!(serde_json::from_value::<PolarLicenseConfig>(value).is_err());
    }
    let mut unknown = valid;
    unknown["endpoint"] = json!("https://arbitrary.invalid");
    assert!(serde_json::from_value::<PolarLicenseConfig>(unknown).is_err());
}

#[test]
fn scope_ids_require_canonical_uuid4() {
    for invalid in [
        "",
        "123",
        " 11111111-1111-4111-8111-111111111111",
        "11111111-1111-4111-8111-111111111111 ",
        "11111111111141118111111111111111",
        "11111111-1111-1111-8111-111111111111",
        "11111111-1111-3111-8111-111111111111",
        "11111111-1111-4111-7111-111111111111",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
        "11111111-1111-4111-8111-11111111111g",
    ] {
        for organization in [true, false] {
            let mut configuration = config();
            if organization {
                configuration.license_scope.organization_id = invalid.into();
            } else {
                configuration.license_scope.benefit_ids = vec![invalid.into()];
            }
            assert_invalid_config(configuration);
        }
    }
}

#[test]
fn benefit_allowlist_requires_unique_ids_and_one_to_128_entries() {
    let mut configuration = config();
    configuration.license_scope.benefit_ids.clear();
    assert_invalid_config(configuration.clone());

    configuration.license_scope.benefit_ids =
        vec!["22222222-2222-4222-8222-222222222222".into(); 2];
    assert_invalid_config(configuration.clone());

    configuration.license_scope.benefit_ids = (0..128)
        .map(|id| format!("00000000-0000-4000-8000-{id:012x}"))
        .collect();
    assert!(PolarLicenseProvider::new(configuration.clone()).is_ok());
    configuration
        .license_scope
        .benefit_ids
        .push("ffffffff-ffff-4fff-8fff-ffffffffffff".into());
    assert_invalid_config(configuration);
}

fn assert_invalid_config(configuration: PolarLicenseConfig) {
    assert_eq!(
        PolarLicenseProvider::new(configuration).err().unwrap().code,
        ErrorCode::InvalidProviderConfiguration
    );
}

#[tokio::test]
async fn execute_and_typed_activation_preserve_invalid_key_errors() {
    let provider = PolarLicenseProvider::new(config()).unwrap();
    let intent: Intent = serde_json::from_value(json!({
        "operation": "activate", "licenseKey": " "
    }))
    .unwrap();
    assert_eq!(
        provider.execute(intent).await.err().unwrap().code,
        ErrorCode::InvalidLicenseKey
    );
    assert_eq!(
        provider.activate(" ").await.unwrap_err().code,
        ErrorCode::InvalidLicenseKey
    );
}

#[test]
fn request_deadline_respects_an_omitted_or_explicit_operation_budget() {
    for environment in ["sandbox", "production"] {
        for (request, operation, accepted) in [
            (45.0, None, true),
            (60.0, None, true),
            (61.0, None, false),
            (61.0, Some(60.0), false),
            (61.0, Some(61.0), true),
            (30.0, Some(29.0), false),
        ] {
            let mut wire = config_wire();
            wire["environment"] = json!(environment);
            wire["requestTimeoutSeconds"] = json!(request);
            if let Some(operation) = operation {
                wire["operationTimeoutSeconds"] = json!(operation);
            }
            let bytes = serde_json::to_vec(&wire).unwrap();
            let configuration: PolarLicenseConfig = software_licensing::decode(&bytes).unwrap();
            assert_eq!(configuration.request_timeout_seconds, request);
            assert_eq!(
                configuration.operation_timeout_seconds,
                operation.unwrap_or(60.0)
            );
            let error = PolarLicenseProvider::new(configuration)
                .err()
                .map(|failure| failure.code);
            assert_eq!(
                error,
                (!accepted).then_some(ErrorCode::InvalidProviderConfiguration),
                "{environment} request {request}, operation {operation:?}"
            );
        }
    }
}

#[test]
fn invalid_configuration_is_rejected_before_transport() {
    for field in [
        "baseRetryDelay",
        "maximumRetryDelay",
        "requestTimeoutSeconds",
        "operationTimeoutSeconds",
    ] {
        for invalid in [0.0, -1.0, 1e-20, 600.1] {
            let mut value = config_wire();
            value[field] = json!(invalid);
            let configuration: PolarLicenseConfig = serde_json::from_value(value).unwrap();
            assert_eq!(
                PolarLicenseProvider::new(configuration).err().unwrap().code,
                ErrorCode::InvalidProviderConfiguration,
                "{field}: {invalid}"
            );
        }
    }
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for field in 0..4 {
            let mut configuration = config();
            match field {
                0 => configuration.base_retry_delay = invalid,
                1 => configuration.maximum_retry_delay = invalid,
                2 => configuration.request_timeout_seconds = invalid,
                _ => configuration.operation_timeout_seconds = invalid,
            }
            assert_eq!(
                configuration.validated().unwrap_err().code,
                ErrorCode::InvalidProviderConfiguration
            );
        }
    }
    let mut configuration = config();
    configuration.request_timeout_seconds = 300.0;
    configuration.operation_timeout_seconds = 600.0;
    configuration.base_retry_delay = 300.0;
    configuration.maximum_retry_delay = 300.0;
    configuration.maximum_request_attempts = 5;
    configuration.validated().unwrap();
    configuration.request_timeout_seconds = 300.1;
    assert!(configuration.validated().is_err());
    configuration = config();
    configuration.base_retry_delay = 31.0;
    assert!(configuration.validated().is_err());
    for attempts in [0, 6, u32::MAX] {
        configuration = config();
        configuration.maximum_request_attempts = attempts;
        assert!(configuration.validated().is_err());
    }
}
