//! Versioned public Polar license operations. No merchant token or billing API.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use software_licensing::{
    Activation, ErrorCode, Failure, MutationOutcome, Operation, ProviderReply, Validation,
    identifier, normalize_key, valid_time,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// The reviewed Polar contract. A removed version must never revoke a saved license.
pub const API_VERSION: &str = "2026-10";
pub(crate) const MAX_RESPONSE_BYTES: usize = 524_288;

fn attempts() -> u32 {
    3
}
fn delay() -> f64 {
    0.2
}
fn maximum_delay() -> f64 {
    30.0
}
fn request_timeout() -> f64 {
    30.0
}
fn operation_timeout() -> f64 {
    60.0
}

/// Explicit environment selection prevents sandbox license keys reaching production.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Production,
    Sandbox,
}
impl Environment {
    fn origin(self) -> &'static str {
        match self {
            Self::Production => "https://api.polar.sh",
            Self::Sandbox => "https://sandbox-api.polar.sh",
        }
    }
}

/// Organization and nonempty benefit allowlist required for every accepted license.
/// IDs use canonical lowercase UUID version 4 with the RFC variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LicenseScope {
    pub organization_id: String,
    pub benefit_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolarLicenseConfig {
    pub activation_instance_name: String,
    pub license_scope: LicenseScope,
    pub environment: Environment,
    #[serde(default = "attempts")]
    pub maximum_request_attempts: u32,
    #[serde(default = "delay")]
    pub base_retry_delay: f64,
    #[serde(default = "maximum_delay")]
    pub maximum_retry_delay: f64,
    /// Includes sending the request and consuming its complete response body.
    #[serde(default = "request_timeout")]
    pub request_timeout_seconds: f64,
    /// Includes every attempt, response body and retry sleep.
    #[serde(default = "operation_timeout")]
    pub operation_timeout_seconds: f64,
}
impl PolarLicenseConfig {
    pub fn validated(&self) -> Result<(), Failure> {
        self.instance_name()?;
        canonical_uuid(&self.license_scope.organization_id).map_err(|_| bad_configuration())?;
        let benefits = &self.license_scope.benefit_ids;
        if benefits.is_empty() || benefits.len() > 128 {
            return Err(bad_configuration());
        }
        for (index, id) in benefits.iter().enumerate() {
            canonical_uuid(id).map_err(|_| bad_configuration())?;
            if benefits[..index].contains(id) {
                return Err(bad_configuration());
            }
        }
        if !(1..=5).contains(&self.maximum_request_attempts)
            || !positive_duration(self.base_retry_delay)
            || !positive_duration(self.maximum_retry_delay)
            || self.base_retry_delay > self.maximum_retry_delay
            || self.maximum_retry_delay > 300.0
            || !positive_duration(self.request_timeout_seconds)
            || !positive_duration(self.operation_timeout_seconds)
            || self.request_timeout_seconds > 300.0
            || self.operation_timeout_seconds > 600.0
            || self.request_timeout_seconds > self.operation_timeout_seconds
        {
            return Err(bad_configuration());
        }
        Ok(())
    }
    fn instance_name(&self) -> Result<String, Failure> {
        identifier(&self.activation_instance_name).map_err(|_| bad_configuration())
    }
}
fn canonical_uuid(value: &str) -> Result<(), Failure> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 || bytes[14] != b'4' || !b"89ab".contains(&bytes[19]) {
        return Err(bad_response());
    }
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return Err(bad_response());
            }
        } else if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte) {
            return Err(bad_response());
        }
    }
    Ok(())
}
fn positive_duration(seconds: f64) -> bool {
    std::time::Duration::try_from_secs_f64(seconds).is_ok_and(|d| !d.is_zero())
}
fn bad_configuration() -> Failure {
    Failure::new(ErrorCode::InvalidProviderConfiguration)
}
fn bad_response() -> Failure {
    Failure::new(ErrorCode::UnexpectedProviderResponse)
}

/// Credential-bearing intent; intentionally does not implement Debug.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    pub operation: Operation,
    pub license_key: Option<String>,
    pub activation: Option<Activation>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HttpRequest {
    pub url: String,
    pub body: String,
    pub headers: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HttpResponse {
    pub status_code: u16,
    pub body: String,
    pub retry_after: Option<String>,
    pub api_version: Option<String>,
}

impl HttpResponse {
    fn matches_api_version(&self) -> bool {
        self.api_version.as_deref() == Some(API_VERSION)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Decision {
    pub reply: Option<ProviderReply>,
    pub retry_delay_seconds: Option<f64>,
    pub retry_failure: Option<Failure>,
}
fn credentials(
    config: &PolarLicenseConfig,
    intent: &Intent,
) -> Result<(String, Option<String>), Failure> {
    if intent.operation == Operation::Activate {
        if intent.activation.is_some() {
            return Err(bad_configuration());
        }
        let key = normalize_key(intent.license_key.as_deref().unwrap_or_default());
        if key.is_empty() || key.len() > 4096 {
            return Err(Failure::new(ErrorCode::InvalidLicenseKey));
        }
        return Ok((key, None));
    }
    let a = intent.activation.as_ref().ok_or_else(bad_configuration)?;
    if a.source != "polar"
        || !config
            .license_scope
            .benefit_ids
            .contains(&a.plan_identifier)
    {
        return Err(Failure::new(ErrorCode::InvalidLicense));
    }
    let a = a.clone().normalized()?;
    let key = a
        .license_key
        .as_ref()
        .filter(|k| !k.is_empty())
        .ok_or_else(bad_configuration)?
        .clone();
    let instance = a
        .activation_identifier
        .as_ref()
        .ok_or_else(bad_configuration)?;
    canonical_uuid(instance).map_err(|_| bad_configuration())?;
    Ok((key, Some(instance.clone())))
}
pub(crate) fn request(
    config: &PolarLicenseConfig,
    intent: &Intent,
) -> Result<HttpRequest, Failure> {
    config.validated()?;
    let (key, instance) = credentials(config, intent)?;
    let mut value = json!({"key": key, "organization_id": config.license_scope.organization_id});
    let operation = match intent.operation {
        Operation::Activate => {
            value["label"] = config.instance_name()?.into();
            "activate"
        }
        Operation::Validate => {
            value["benefit_id"] = intent
                .activation
                .as_ref()
                .ok_or_else(bad_configuration)?
                .plan_identifier
                .clone()
                .into();
            "validate"
        }
        Operation::Deactivate => "deactivate",
    };
    if let Some(instance) = instance {
        value["activation_id"] = instance.into();
    }
    Ok(HttpRequest {
        url: format!(
            "{}/v1/customer-portal/license-keys/{operation}",
            config.environment.origin()
        ),
        body: value.to_string(),
        headers: [
            ("Accept".into(), "application/json".into()),
            ("Content-Type".into(), "application/json".into()),
            ("Polar-Version".into(), API_VERSION.into()),
        ]
        .into(),
    })
}
/// A successful activation retains its device identity even if expiration occurs
/// while the response is in flight. The core persists it without granting access.
pub(crate) fn respond(
    config: &PolarLicenseConfig,
    intent: &Intent,
    response: HttpResponse,
    attempt: u32,
    now: f64,
) -> Result<Decision, Failure> {
    config.validated()?;
    credentials(config, intent)?;
    if response.body.len() > MAX_RESPONSE_BYTES
        || !(100..=599).contains(&response.status_code)
        || attempt == 0
        || attempt > config.maximum_request_attempts
    {
        return Err(bad_response());
    }
    // Versioned response evidence survives an unavailable local wall clock.
    let now = valid_time(now).is_ok().then_some(now);
    let retry_advice = retry_after(response.retry_after.as_deref(), now);
    let read_only = intent.operation == Operation::Validate;
    let retryable = (response.status_code == 429 && (read_only || response.matches_api_version()))
        || (read_only && (500..=599).contains(&response.status_code));
    if retryable
        && attempt < config.maximum_request_attempts
        && let Ok(retry_advice) = retry_advice
    {
        let wait =
            retry_advice.unwrap_or(config.base_retry_delay * 2f64.powi((attempt - 1) as i32));
        if wait <= config.maximum_retry_delay {
            return Ok(Decision {
                reply: None,
                retry_delay_seconds: Some(wait),
                retry_failure: Some(http_failure(&response, None, Some(wait), intent.operation)),
            });
        }
    }
    let value = serde_json::from_str::<Value>(&response.body).ok();
    let result = if !(200..=299).contains(&response.status_code) {
        Err(http_failure(
            &response,
            value.as_ref(),
            retry_advice.unwrap_or_default(),
            intent.operation,
        ))
    } else if !response.matches_api_version() {
        Err(bad_response())
    } else if intent.operation == Operation::Deactivate {
        if response.status_code == 204 && response.body.is_empty() {
            Ok(ProviderReply::Deactivated)
        } else {
            Err(bad_response())
        }
    } else if response.status_code != 200 {
        Err(bad_response())
    } else {
        value
            .as_ref()
            .ok_or_else(bad_response)
            .and_then(|value| parse(config, intent, value, now))
    };
    Ok(Decision {
        reply: Some(result.unwrap_or_else(|mut failure| {
            if intent.operation != Operation::Validate && failure.mutation_outcome.is_none() {
                failure.mutation_outcome = Some(MutationOutcome::Unknown);
            }
            ProviderReply::Failed { failure }
        })),
        retry_delay_seconds: None,
        retry_failure: None,
    })
}
fn parse(
    config: &PolarLicenseConfig,
    intent: &Intent,
    value: &Value,
    now: Option<f64>,
) -> Result<ProviderReply, Failure> {
    let (key, submitted_instance) = credentials(config, intent)?;
    let license = if intent.operation == Operation::Activate {
        value.get("license_key").ok_or_else(bad_response)?
    } else {
        value
    };
    let license_id = uuid_field(license, "id")?;
    let org = uuid_field(license, "organization_id")?;
    let benefit = uuid_field(license, "benefit_id")?;
    let identity_failure = || {
        if intent.operation == Operation::Validate {
            bad_response()
        } else {
            Failure::new(ErrorCode::InvalidLicense)
        }
    };
    if org != config.license_scope.organization_id
        || !config
            .license_scope
            .benefit_ids
            .iter()
            .any(|s| s == benefit)
        || license.get("key").and_then(Value::as_str) != Some(key.as_str())
    {
        return Err(identity_failure());
    }
    let instance = if intent.operation == Operation::Activate {
        value
    } else {
        value.get("activation").ok_or_else(bad_response)?
    };
    let instance_id = uuid_field(instance, "id")?;
    if uuid_field(instance, "license_key_id")? != license_id
        || submitted_instance
            .as_deref()
            .is_some_and(|expected| expected != instance_id)
        || (intent.operation == Operation::Validate
            && intent
                .activation
                .as_ref()
                .is_some_and(|a| a.plan_identifier != benefit))
    {
        return Err(identity_failure());
    }
    match license.get("status").and_then(Value::as_str) {
        Some("granted") => {}
        Some("revoked" | "disabled") => return Err(Failure::new(ErrorCode::InvalidLicense)),
        _ => return Err(bad_response()),
    }
    // Both subscriptions and perpetual purchases may have null expiry. Never infer
    // product tier or lifetime ownership from this field; benefit identity owns that.
    let expires_at = date(license.get("expires_at").ok_or_else(bad_response)?)?;
    match intent.operation {
        Operation::Activate => {
            let activated_at = date(instance.get("created_at").ok_or_else(bad_response)?)?
                .ok_or_else(bad_response)?;
            Ok(ProviderReply::Activated {
                activation: Activation {
                    source: "polar".into(),
                    plan_identifier: benefit.into(),
                    activated_at,
                    license_key: Some(key),
                    activation_identifier: Some(instance_id.into()),
                    expires_at,
                }
                .normalized()
                .map_err(|_| bad_response())?,
            })
        }
        Operation::Validate => {
            if let Some(expires_at) = expires_at
                && expires_at <= now.ok_or_else(bad_response)?
            {
                return Err(Failure::new(ErrorCode::ExpiredLicense));
            }
            Ok(ProviderReply::Validated {
                validation: Validation {
                    is_valid: true,
                    plan_identifier: Some(benefit.into()),
                    expires_at,
                },
            })
        }
        Operation::Deactivate => unreachable!(),
    }
}
fn uuid_field<'a>(value: &'a Value, name: &str) -> Result<&'a str, Failure> {
    let value = value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(bad_response)?;
    canonical_uuid(value)?;
    Ok(value)
}
fn date(value: &Value) -> Result<Option<f64>, Failure> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => {
            let date = OffsetDateTime::parse(s, &Rfc3339).map_err(|_| bad_response())?;
            let seconds = date.unix_timestamp() as f64 + f64::from(date.nanosecond()) / 1e9;
            valid_time(seconds).map_err(|_| bad_response())?;
            Ok(Some(seconds))
        }
        _ => Err(bad_response()),
    }
}
fn http_failure(
    response: &HttpResponse,
    value: Option<&Value>,
    retry_delay: Option<f64>,
    operation: Operation,
) -> Failure {
    let status = response.status_code;
    let typed = response.matches_api_version();
    let error = value.and_then(|v| v.get("error")).and_then(Value::as_str);
    // Unsupported/removed API versions also produce 404 without Polar-Version.
    // Generic proxy/router errors cannot establish the result of a license operation.
    let code = if status == 429 {
        ErrorCode::RateLimited
    } else if status >= 500 {
        ErrorCode::ServerFailure
    } else if typed
        && ((status == 404 && error == Some("ResourceNotFound"))
            || (status == 403 && error == Some("NotPermitted")))
    {
        ErrorCode::InvalidLicense
    } else {
        ErrorCode::RequestFailure
    };
    let mut failure = Failure::new(code);
    failure.status_code = Some(status);
    failure.retry_after_seconds = retry_delay.map(|d| d.ceil() as u64);
    if operation != Operation::Validate {
        failure.mutation_outcome = Some(if typed && (400..500).contains(&status) {
            MutationOutcome::NotApplied
        } else {
            MutationOutcome::Unknown
        });
    }
    failure
}
// A valid absolute date without a clock blocks automatic retry. Missing or
// malformed advice still allows the configured backoff.
fn retry_after(value: Option<&str>, now: Option<f64>) -> Result<Option<f64>, ()> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Some(value.parse::<f64>().unwrap_or(f64::INFINITY)));
    }
    let Some(date) = httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
    else {
        return Ok(None);
    };
    Ok(Some((date.as_secs_f64() - now.ok_or(())?).max(0.0)))
}
