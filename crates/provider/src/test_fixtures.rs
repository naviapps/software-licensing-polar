use crate::{Intent, PolarLicenseConfig};
use serde_json::{Value, json};
use software_licensing::{Activation, Operation};

pub const ORGANIZATION: &str = "11111111-1111-4111-8111-111111111111";
pub const BENEFIT: &str = "22222222-2222-4222-8222-222222222222";
pub const INSTANCE: &str = "44444444-4444-4444-8444-444444444444";
pub const OTHER: &str = "55555555-5555-4555-8555-555555555555";
pub const CREATED: f64 = 1_700_000_000.0;
const LICENSE: &str = "33333333-3333-4333-8333-333333333333";

pub fn config() -> PolarLicenseConfig {
    serde_json::from_value(json!({
        "environment": "sandbox",
        "activationInstanceName": "Fixture",
        "licenseScope": { "organizationId": ORGANIZATION, "benefitIds": [BENEFIT] },
    }))
    .unwrap()
}

pub fn activation() -> Activation {
    Activation {
        source: "polar".into(),
        plan_identifier: BENEFIT.into(),
        activated_at: CREATED,
        license_key: Some("TEST-KEY".into()),
        activation_identifier: Some(INSTANCE.into()),
        expires_at: None,
    }
}

pub fn intent(operation: Operation) -> Intent {
    Intent {
        operation,
        license_key: (operation == Operation::Activate).then(|| "TEST-KEY".into()),
        activation: (operation != Operation::Activate).then(activation),
    }
}

pub fn granted_response(operation: Operation) -> Value {
    let mut key = json!({
        "id": LICENSE, "organization_id": ORGANIZATION, "benefit_id": BENEFIT,
        "key": "TEST-KEY", "status": "granted", "expires_at": null,
        "limit_activations": 5, "usage": 0, "limit_usage": null, "validations": 1,
        "last_validated_at": null,
    });
    let mut instance = json!({
        "id": INSTANCE, "license_key_id": LICENSE, "label": "Fixture", "meta": {},
        "created_at": "2023-11-14T22:13:20Z", "modified_at": null,
    });
    if operation == Operation::Activate {
        instance["license_key"] = key;
        instance
    } else {
        key["activation"] = instance;
        key
    }
}
