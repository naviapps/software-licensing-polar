//! Real HTTP against owned loopback sockets. No external provider requests.
use crate::{
    API_VERSION, Intent, PolarLicenseConfig, PolarLicenseProvider, protocol::MAX_RESPONSE_BYTES,
};
use serde_json::{Value, json};
use software_licensing::{
    Activation, Command, Effect, ErrorCode, Failure, LicenseRefreshOutcome, MutationOutcome,
    Operation, Policy, ProviderReply, Record, Reply, Session, Status,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};

struct Captured {
    request: String,
    body: String,
    at: Instant,
    wall_time: SystemTime,
}
struct Response {
    bytes: Vec<u8>,
    split: usize,
    before_headers: Duration,
    before_body: Duration,
}
impl Response {
    fn raw(bytes: Vec<u8>) -> Self {
        Self {
            split: bytes.len(),
            bytes,
            before_headers: Duration::ZERO,
            before_body: Duration::ZERO,
        }
    }
    fn http(status: u16, headers: &str, body: impl AsRef<[u8]>) -> Self {
        let body = body.as_ref();
        let mut bytes = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nPolar-Version: {API_VERSION}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        let split = bytes.len();
        bytes.extend_from_slice(body);
        Self {
            split,
            ..Self::raw(bytes)
        }
    }
    fn success() -> Self {
        Self::success_for(Operation::Activate)
    }
    fn success_for(operation: Operation) -> Self {
        if operation == Operation::Deactivate {
            Self::http(204, "", "")
        } else {
            Self::http(200, "", valid_for(operation).to_string())
        }
    }
    fn delayed_headers(mut self) -> Self {
        self.before_headers = Duration::from_millis(150);
        self
    }
    fn delayed_body(mut self) -> Self {
        self.before_body = Duration::from_millis(150);
        self
    }
}
struct Fixture {
    endpoint: String,
    received: mpsc::UnboundedReceiver<Captured>,
    count: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}
impl Fixture {
    async fn new(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/customer-portal/license-keys",
            listener.local_addr().unwrap()
        );
        let (sender, received) = mpsc::unbounded_channel();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let task = tokio::spawn(async move {
            let mut responses: VecDeque<_> = responses.into();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let captured = read_request(&mut stream).await;
                observed.fetch_add(1, Ordering::SeqCst);
                if sender.send(captured).is_err() {
                    return;
                }
                // Keep accepting unexpected attempts after the scripted replies.
                let response = responses
                    .pop_front()
                    .unwrap_or_else(|| Response::http(500, "", ""));
                sleep(response.before_headers).await;
                let _ = stream.write_all(&response.bytes[..response.split]).await;
                sleep(response.before_body).await;
                let _ = stream.write_all(&response.bytes[response.split..]).await;
                let _ = stream.shutdown().await;
            }
        });
        Self {
            endpoint,
            received,
            count,
            task,
        }
    }
    fn provider(&self, config: PolarLicenseConfig) -> PolarLicenseProvider {
        PolarLicenseProvider::for_test(config, self.endpoint.clone()).unwrap()
    }
    fn provider_with_invalid_wall_time(&self) -> PolarLicenseProvider {
        let mut provider = self.provider(config());
        provider.test_wall_time = Some(UNIX_EPOCH + Duration::from_secs(253_402_300_800));
        provider
    }
    async fn next(&mut self) -> Captured {
        timeout(Duration::from_secs(2), self.received.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn assert_attempts(&self, expected: usize) {
        sleep(Duration::from_millis(220)).await;
        assert_eq!(self.count.load(Ordering::SeqCst), expected);
        assert!(!self.task.is_finished(), "fixture failed");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn read_request(stream: &mut TcpStream) -> Captured {
    let mut bytes = Vec::new();
    let end = loop {
        let mut buffer = [0; 1024];
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        assert!(bytes.len() < 65536);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let request = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length: usize = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    let mut body = bytes[end..].to_vec();
    let read = body.len();
    body.resize(length, 0);
    stream.read_exact(&mut body[read..]).await.unwrap();
    Captured {
        request,
        body: String::from_utf8(body).unwrap(),
        at: Instant::now(),
        wall_time: SystemTime::now(),
    }
}
const ORGANIZATION: &str = "11111111-1111-4111-8111-111111111111";
const BENEFIT: &str = "22222222-2222-4222-8222-222222222222";
const LICENSE: &str = "33333333-3333-4333-8333-333333333333";
const ACTIVATION: &str = "44444444-4444-4444-8444-444444444444";
const OTHER: &str = "99999999-9999-4999-8999-999999999999";

fn config() -> PolarLicenseConfig {
    serde_json::from_value(json!({
        "environment":"sandbox",
        "activationInstanceName":"Fixture",
        "licenseScope":{"organizationId":ORGANIZATION,"benefitIds":[BENEFIT]},
        "baseRetryDelay":0.01,"maximumRetryDelay":2
    }))
    .unwrap()
}
fn activation() -> Activation {
    Activation {
        source: "polar".into(),
        plan_identifier: BENEFIT.into(),
        activated_at: 1700000000.0,
        license_key: Some("TEST-KEY".into()),
        activation_identifier: Some(ACTIVATION.into()),
        expires_at: None,
    }
}
fn intent(operation: Operation) -> Intent {
    Intent {
        operation,
        license_key: (operation == Operation::Activate).then(|| "TEST-KEY".into()),
        activation: (operation != Operation::Activate).then(activation),
    }
}
fn valid() -> Value {
    valid_for(Operation::Activate)
}
fn valid_for(operation: Operation) -> Value {
    let key = json!({
        "id":LICENSE,"organization_id":ORGANIZATION,"benefit_id":BENEFIT,
        "key":"TEST-KEY","status":"granted","expires_at":null,
        "limit_activations":5,"usage":0,"limit_usage":null,"validations":1,
        "last_validated_at":null
    });
    let activation = json!({
        "id":ACTIVATION,"license_key_id":LICENSE,"label":"Fixture","meta":{},
        "created_at":"2023-11-14T22:13:20Z","modified_at":null
    });
    if operation == Operation::Activate {
        let mut value = activation;
        value["license_key"] = key;
        value
    } else {
        let mut value = key;
        value["activation"] = activation;
        value
    }
}
fn failure<T>(result: Result<T, Failure>) -> Failure {
    match result {
        Err(failure) => failure,
        _ => panic!("expected failure"),
    }
}

#[tokio::test]
async fn malformed_or_slow_rejection_bodies_retain_http_evidence() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for status in [400, 404] {
            for body_case in 0..4 {
                let response = match body_case {
                    0 => Response::http(status, "Retry-After: 5\r\n", [0xff]),
                    1 => Response::raw(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 8\r\nPolar-Version: {API_VERSION}\r\nRetry-After: 5\r\nConnection: close\r\n\r\n{{}}").into_bytes()),
                    2 => Response::raw(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nPolar-Version: {API_VERSION}\r\nRetry-After: 5\r\nConnection: close\r\n\r\n", MAX_RESPONSE_BYTES + 1).into_bytes()),
                    _ => {
                        let mut response = Response::http(status, "Retry-After: 5\r\n", "{}");
                        response.before_body = Duration::from_secs(1);
                        response
                    }
                };
                let fixture = Fixture::new(vec![response]).await;
                let mut configuration = config();
                configuration.request_timeout_seconds = 0.5;
                configuration.operation_timeout_seconds = 1.0;
                let error = failure(
                    fixture
                        .provider(configuration)
                        .execute(intent(operation))
                        .await,
                );
                assert_eq!(
                    error.status_code,
                    Some(status),
                    "{operation:?}/{status}/{body_case}"
                );
                assert_eq!(error.code, ErrorCode::RequestFailure);
                assert_eq!(error.retry_after_seconds, Some(5));
                assert_eq!(
                    error.mutation_outcome,
                    if operation == Operation::Validate {
                        None
                    } else {
                        Some(software_licensing::MutationOutcome::NotApplied)
                    }
                );
                assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
            }
        }
    }
}

#[tokio::test]
async fn cancelling_a_retry_wait_retains_the_confirmed_rejection() {
    for (operation, status) in [
        (Operation::Activate, 429),
        (Operation::Validate, 429),
        (Operation::Deactivate, 429),
        (Operation::Validate, 503),
    ] {
        let fixture = Fixture::new(vec![Response::http(status, "Retry-After: 1\r\n", "")]).await;
        let mut provider = fixture.provider(config());
        let retry = Arc::new(tokio::sync::Notify::new());
        provider.retry_wait = Some(retry.clone());
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        let execution = provider.execute_with_cancellation(intent(operation), async {
            let _ = cancelled.await;
        });
        tokio::pin!(execution);
        tokio::select! {
            _ = &mut execution => panic!("completed before retry wait"),
            _ = retry.notified() => {},
        }
        cancel.send(()).unwrap();
        let error = failure(execution.await);
        assert_eq!(
            error.code,
            if status == 429 {
                ErrorCode::RateLimited
            } else {
                ErrorCode::ServerFailure
            }
        );
        assert_eq!(error.status_code, Some(status));
        assert_eq!(error.retry_after_seconds, Some(1));
        assert_eq!(
            error.mutation_outcome,
            if operation == Operation::Validate {
                None
            } else {
                Some(software_licensing::MutationOutcome::NotApplied)
            }
        );
        sleep(Duration::from_millis(1050)).await;
        assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancellation_before_send_is_not_applied_but_an_unconfirmed_send_is_unknown() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        let fixture = Fixture::new(vec![Response::success()]).await;
        let provider = fixture.provider(config());
        let error = failure(
            provider
                .execute_with_cancellation(intent(operation), std::future::ready(()))
                .await,
        );
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            error.mutation_outcome,
            if operation == Operation::Validate {
                None
            } else {
                Some(software_licensing::MutationOutcome::NotApplied)
            }
        );
        assert_eq!(fixture.count.load(Ordering::SeqCst), 0);

        let mut fixture = Fixture::new(vec![Response::success().delayed_headers()]).await;
        let provider = fixture.provider(config());
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        let execution = provider.execute_with_cancellation(intent(operation), async {
            let _ = cancelled.await;
        });
        tokio::pin!(execution);
        tokio::select! {
            _ = &mut execution => panic!("completed before request reached fixture"),
            _ = fixture.next() => {},
        }
        cancel.send(()).unwrap();
        let error = failure(execution.await);
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            error.mutation_outcome,
            if operation == Operation::Validate {
                None
            } else {
                Some(software_licensing::MutationOutcome::Unknown)
            }
        );
        fixture.assert_attempts(1).await;
    }
}

#[tokio::test]
async fn json_utf8_special_characters_and_all_operation_paths() {
    let mut value = valid();
    value["license_key"]["key"] = json!("a&b+c%=漢字");
    let mut fixture = Fixture::new(vec![
        Response::http(200, "", value.to_string()),
        Response::success_for(Operation::Validate),
        Response::success_for(Operation::Deactivate),
    ])
    .await;
    let mut config = config();
    config.activation_instance_name = "端末 & +%= name".into();
    let provider = fixture.provider(config);
    let activated: Activation = provider.activate(" a&b+c%=漢字 ").await.unwrap();
    assert_eq!(activated.license_key.as_deref(), Some("a&b+c%=漢字"));
    let request = fixture.next().await;
    assert!(
        request
            .request
            .starts_with("POST /v1/customer-portal/license-keys/activate HTTP/1.1\r\n")
    );
    let headers = request.request.to_ascii_lowercase();
    assert!(headers.contains("content-type: application/json\r\n"));
    assert!(headers.contains("accept: application/json\r\n"));
    assert!(headers.contains("polar-version: 2026-10\r\n"));
    assert!(!headers.contains("authorization:"));
    let fields: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(fields["key"], "a&b+c%=漢字");
    assert_eq!(fields["label"], "端末 & +%= name");
    assert_eq!(fields["organization_id"], ORGANIZATION);
    assert!(fields.get("increment_usage").is_none());
    assert!(provider.validate(activation()).await.unwrap().is_valid);
    let request = fixture.next().await;
    assert!(
        request
            .request
            .starts_with("POST /v1/customer-portal/license-keys/validate ")
    );
    let fields: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(fields["key"], "TEST-KEY");
    assert_eq!(fields["organization_id"], ORGANIZATION);
    assert_eq!(fields["activation_id"], ACTIVATION);
    assert!(fields.get("increment_usage").is_none());
    provider.deactivate(activation()).await.unwrap();
    let request = fixture.next().await;
    assert!(
        request
            .request
            .starts_with("POST /v1/customer-portal/license-keys/deactivate ")
    );
    let fields: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(fields["key"], "TEST-KEY");
    assert_eq!(fields["organization_id"], ORGANIZATION);
    assert_eq!(fields["activation_id"], ACTIVATION);
}

#[tokio::test]
async fn redirects_are_not_followed_for_any_operation() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for status in [301, 302, 303, 307, 308] {
            // Any redirect replay would reach this second owned listener.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let destination = format!("http://{}/stolen", listener.local_addr().unwrap());
            let fixture = Fixture::new(vec![Response::http(
                status,
                &format!("Location: {destination}\r\n"),
                "PRIVATE-KEY",
            )])
            .await;
            assert_eq!(
                failure(fixture.provider(config()).execute(intent(operation)).await).code,
                ErrorCode::RequestFailure
            );
            assert!(
                timeout(Duration::from_millis(15), listener.accept())
                    .await
                    .is_err()
            );
            assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn bounded_content_length_and_streamed_bodies() {
    let mut padded = valid().to_string().into_bytes();
    padded.resize(MAX_RESPONSE_BYTES, b' ');
    let fixture = Fixture::new(vec![Response::http(200, "", padded)]).await;
    fixture
        .provider(config())
        .activate("TEST-KEY")
        .await
        .unwrap();

    for response in [
        Response::http(200, "", vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Response::raw(
            format!(
                "HTTP/1.1 200 OK\r\nPolar-Version: 2026-10\r\nContent-Length: {}\r\n\r\n",
                MAX_RESPONSE_BYTES + 1
            )
            .into_bytes(),
        ),
        Response::raw({
            let mut bytes =
                b"HTTP/1.1 200 OK\r\nPolar-Version: 2026-10\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                    .to_vec();
            for size in [MAX_RESPONSE_BYTES, 1] {
                bytes.extend_from_slice(format!("{size:x}\r\n").as_bytes());
                bytes.extend(std::iter::repeat_n(b' ', size));
                bytes.extend_from_slice(b"\r\n");
            }
            bytes.extend_from_slice(b"0\r\n\r\n");
            bytes
        }),
    ] {
        let fixture = Fixture::new(vec![response]).await;
        assert_eq!(
            failure(fixture.provider(config()).activate("TEST-KEY").await).code,
            ErrorCode::UnexpectedProviderResponse
        );
        assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn missing_or_invalid_expiration_fails_without_replaying_successful_requests() {
    for operation in [Operation::Activate, Operation::Validate] {
        for expiration in [None, Some(json!("not a date"))] {
            let mut value = valid_for(operation);
            let key = if operation == Operation::Activate {
                &mut value["license_key"]
            } else {
                &mut value
            };
            match expiration {
                Some(expiration) => key["expires_at"] = expiration,
                None => {
                    key.as_object_mut().unwrap().remove("expires_at");
                }
            }
            let fixture = Fixture::new(vec![
                Response::http(200, "", value.to_string()),
                Response::success(),
            ])
            .await;
            let error = failure(fixture.provider(config()).execute(intent(operation)).await);
            assert_eq!(error.code, ErrorCode::UnexpectedProviderResponse);
            assert_eq!(
                error.mutation_outcome,
                (operation == Operation::Activate)
                    .then_some(software_licensing::MutationOutcome::Unknown)
            );
            fixture.assert_attempts(1).await;
        }
    }
}

#[tokio::test]
async fn validation_retries_5xx_but_mutations_never_do() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        let fixture = Fixture::new(vec![
            Response::http(503, "", ""),
            Response::success_for(operation),
        ])
        .await;
        let result = fixture.provider(config()).execute(intent(operation)).await;
        if operation == Operation::Validate {
            assert!(matches!(result.unwrap(), ProviderReply::Validated { .. }));
            assert_eq!(fixture.count.load(Ordering::SeqCst), 2);
        } else {
            assert_eq!(
                failure(result),
                Failure {
                    code: ErrorCode::ServerFailure,
                    status_code: Some(503),
                    mutation_outcome: Some(software_licensing::MutationOutcome::Unknown),
                    ..Failure::new(ErrorCode::ServerFailure)
                }
            );
            fixture.assert_attempts(1).await;
        }
    }
}

#[tokio::test]
async fn rate_limits_retry_all_operations_without_cookies_and_stop_at_attempt_bound() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        let mut fixture = Fixture::new(vec![
            Response::http(429, "Retry-After: 0\r\nSet-Cookie: secret=fixture\r\n", ""),
            Response::success_for(operation),
        ])
        .await;
        let result = fixture
            .provider(config())
            .execute(intent(operation))
            .await
            .unwrap();
        assert!(matches!(
            (operation, result),
            (Operation::Activate, ProviderReply::Activated { .. })
                | (Operation::Validate, ProviderReply::Validated { .. })
                | (Operation::Deactivate, ProviderReply::Deactivated)
        ));
        fixture.next().await;
        assert!(
            !fixture
                .next()
                .await
                .request
                .to_ascii_lowercase()
                .contains("cookie:")
        );
        assert_eq!(fixture.count.load(Ordering::SeqCst), 2);
    }
    let fixture = Fixture::new(
        (0..3)
            .map(|_| Response::http(429, "Retry-After: 0\r\n", "PRIVATE-KEY"))
            .collect(),
    )
    .await;
    let failure = failure(fixture.provider(config()).activate("TEST-KEY").await);
    assert_eq!(
        failure,
        Failure {
            code: ErrorCode::RateLimited,
            status_code: Some(429),
            retry_after_seconds: Some(0),
            mutation_outcome: Some(software_licensing::MutationOutcome::NotApplied),
        }
    );
    fixture.assert_attempts(3).await;
}

#[tokio::test]
async fn invalid_wall_time_preserves_uncertain_activation_after_reopen() {
    let now = 1_700_000_100.0;
    let mut fixture = Fixture::new(vec![Response::http(200, "", "{")]).await;
    let provider = fixture.provider_with_invalid_wall_time();
    let mut session = Session::default();
    session
        .dispatch(Command::Restore {
            record: None,
            policy: Policy::default(),
            now,
        })
        .unwrap();
    let start = session
        .dispatch(Command::Activate {
            license_key: Some("TEST-KEY".into()),
            now,
        })
        .unwrap();
    let Some(Effect::Persist { id, record }) = start.effect else {
        panic!("Activation must persist its intent before contacting Polar");
    };
    assert!(record.pending_activation.is_some());
    let started = session
        .dispatch(Command::Persisted {
            id,
            success: true,
            now,
        })
        .unwrap();
    let Some(Effect::Provider {
        id,
        operation: Operation::Activate,
        license_key,
        activation: None,
    }) = started.effect
    else {
        panic!("Expected a Polar activation");
    };
    let failure = failure(
        provider
            .execute(Intent {
                operation: Operation::Activate,
                license_key,
                activation: None,
            })
            .await,
    );
    assert!(
        fixture
            .next()
            .await
            .request
            .starts_with("POST /v1/customer-portal/license-keys/activate ")
    );
    let finished = session
        .dispatch(Command::Provider {
            id,
            result: ProviderReply::Failed {
                failure: failure.clone(),
            },
            now,
        })
        .unwrap();
    let Some(Effect::Persist { id, record }) = finished.effect else {
        panic!("The provider outcome must be saved");
    };
    assert!(record.pending_activation.is_some(), "{failure:?}");
    assert_eq!(failure.code, ErrorCode::UnexpectedProviderResponse);
    assert_eq!(failure.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(record.last_operation_failure, Some(failure));
    let finished = session
        .dispatch(Command::Persisted {
            id,
            success: true,
            now,
        })
        .unwrap();
    assert_eq!(
        finished.error.unwrap().code,
        ErrorCode::MutationOutcomeUnknown
    );

    let mut restored = Session::default();
    let reopened = restored
        .dispatch(Command::Restore {
            record: Some(serde_json::to_value(record).unwrap()),
            policy: Policy::default(),
            now,
        })
        .unwrap();
    assert!(reopened.state.has_pending_activation);
    assert_eq!(
        restored
            .dispatch(Command::Activate {
                license_key: Some("TEST-KEY".into()),
                now,
            })
            .unwrap_err()
            .code,
        ErrorCode::ActivationPending
    );
    fixture.assert_attempts(1).await;
}

#[tokio::test]
async fn invalid_wall_time_only_blocks_clock_dependent_decisions() {
    let mut expiring = valid_for(Operation::Validate);
    expiring["expires_at"] = json!("2035-01-01T00:00:00Z");
    let retry_date = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(60));
    let fixture = Fixture::new(vec![
        Response::success(),
        Response::success_for(Operation::Deactivate),
        Response::success_for(Operation::Validate),
        Response::http(200, "", expiring.to_string()),
        Response::http(403, "", json!({"error": "NotPermitted"}).to_string()),
        Response::http(429, &format!("Retry-After: {retry_date}\r\n"), ""),
        Response::http(429, "Retry-After: 0\r\n", ""),
        Response::success(),
    ])
    .await;
    let provider = fixture.provider_with_invalid_wall_time();

    let saved = provider.activate("TEST-KEY").await.unwrap();
    assert_eq!(saved.activation_identifier.as_deref(), Some(ACTIVATION));
    provider.deactivate(saved.clone()).await.unwrap();
    assert!(provider.validate(saved.clone()).await.unwrap().is_valid);
    assert_eq!(
        failure(provider.validate(saved).await),
        Failure::new(ErrorCode::UnexpectedProviderResponse)
    );
    assert_eq!(
        failure(provider.activate("TEST-KEY").await),
        Failure {
            code: ErrorCode::InvalidLicense,
            status_code: Some(403),
            retry_after_seconds: None,
            mutation_outcome: Some(MutationOutcome::NotApplied),
        }
    );
    assert_eq!(
        failure(provider.activate("TEST-KEY").await),
        Failure {
            code: ErrorCode::RateLimited,
            status_code: Some(429),
            retry_after_seconds: None,
            mutation_outcome: Some(MutationOutcome::NotApplied),
        }
    );
    provider.activate("TEST-KEY").await.unwrap();
    fixture.assert_attempts(8).await;
}

#[tokio::test]
async fn durable_release_distinguishes_interruption_from_a_polar_retry_deadline() {
    let now = 1_700_000_100.0;
    let saved = activation();
    for rate_limited in [false, true] {
        let mut responses = vec![Response::success_for(Operation::Deactivate)];
        if rate_limited {
            responses.insert(0, Response::http(429, "Retry-After: 120\r\n", ""));
        }
        let mut fixture = Fixture::new(responses).await;
        let provider = fixture.provider(config());
        let record = Record {
            activation: Some(saved.clone()),
            status: Status::Active,
            last_validated_at: Some(saved.activated_at),
            ..Record::default()
        };
        let mut original = Session::default();
        let active = original
            .dispatch(Command::Restore {
                record: Some(serde_json::to_value(record).unwrap()),
                policy: Policy::default(),
                now,
            })
            .unwrap();
        assert!(active.state.next_refresh_at.unwrap() > now);
        let prepared = original.dispatch(Command::Deactivate { now }).unwrap();
        let Some(Effect::Persist { id, mut record }) = prepared.effect else {
            panic!("Release must save its journal before contacting Polar");
        };
        assert!(record.pending_deactivation);
        assert_eq!(record.next_refresh_at, None);
        let (restart, due_at) = if rate_limited {
            let started = original
                .dispatch(Command::Persisted {
                    id,
                    success: true,
                    now,
                })
                .unwrap();
            let rejected = finish_polar_release(&mut original, &provider, started, now).await;
            let Some(Effect::Persist {
                record: after_rejection,
                ..
            }) = rejected.effect
            else {
                panic!("A rejected release must save its retry advice");
            };
            assert_eq!(
                after_rejection.last_operation_failure,
                Some(Failure {
                    code: ErrorCode::RateLimited,
                    status_code: Some(429),
                    retry_after_seconds: Some(120),
                    mutation_outcome: Some(MutationOutcome::NotApplied),
                })
            );
            record = after_rejection;
            (now + 119.0, Some(now + 120.0))
        } else {
            (now + 1.0, None)
        };
        assert_eq!(record.activation.as_ref(), Some(&saved));

        let mut restored = Session::default();
        let reopened = restored
            .dispatch(Command::Restore {
                record: Some(serde_json::to_value(record).unwrap()),
                policy: Policy::default(),
                now: restart,
            })
            .unwrap();
        assert!(reopened.state.has_pending_deactivation && !reopened.state.plan.is_licensed);
        assert_eq!(reopened.state.next_refresh_at, due_at);
        assert_eq!(reopened.needs_refresh, due_at.is_none());
        if rate_limited {
            let early = restored
                .dispatch(Command::Refresh {
                    only_if_due: true,
                    now: restart,
                })
                .unwrap();
            assert_eq!(early.outcome, Some(LicenseRefreshOutcome::SkippedNotDue));
            assert!(early.effect.is_none());
        }
        assert_eq!(
            fixture.count.load(Ordering::SeqCst),
            usize::from(rate_limited)
        );
        let now = due_at.unwrap_or(restart);
        let resumed = restored
            .dispatch(Command::Refresh {
                only_if_due: true,
                now,
            })
            .unwrap();
        let confirmed = finish_polar_release(&mut restored, &provider, resumed, now).await;
        let Some(Effect::Persist { id, record }) = confirmed.effect else {
            panic!("The confirmed release must durably clear the activation");
        };
        assert!(record.activation.is_none() && !record.pending_deactivation);
        let committed = restored
            .dispatch(Command::Persisted {
                id,
                success: true,
                now,
            })
            .unwrap();
        assert_eq!(committed.outcome, Some(LicenseRefreshOutcome::Deactivated));
        assert!(!committed.state.has_activation && !committed.state.has_pending_deactivation);
        for _ in 0..=usize::from(rate_limited) {
            let request = fixture.next().await;
            assert!(
                request
                    .request
                    .starts_with("POST /v1/customer-portal/license-keys/deactivate ")
            );
            assert_eq!(
                serde_json::from_str::<Value>(&request.body).unwrap()["activation_id"],
                ACTIVATION
            );
        }
        fixture.assert_attempts(1 + usize::from(rate_limited)).await;
    }
}

async fn finish_polar_release(
    session: &mut Session,
    provider: &PolarLicenseProvider,
    started: Reply,
    now: f64,
) -> Reply {
    let Some(Effect::Provider {
        id,
        operation: Operation::Deactivate,
        license_key: None,
        activation: Some(activation),
    }) = started.effect
    else {
        panic!("Expected a Polar device release without validation");
    };
    let result = match provider.deactivate(activation).await {
        Ok(()) => ProviderReply::Deactivated,
        Err(failure) => ProviderReply::Failed { failure },
    };
    session
        .dispatch(Command::Provider { id, result, now })
        .unwrap()
}

#[tokio::test]
async fn retry_after_seconds_and_http_date_are_respected() {
    for http_date in [false, true] {
        // Generate this case's deadline after the previous case finishes. HTTP
        // dates have whole-second precision and use wall time, not elapsed time.
        let header = if http_date {
            httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(2))
        } else {
            "1".to_owned()
        };
        let deadline = http_date.then(|| httpdate::parse_http_date(&header).unwrap());
        let mut fixture = Fixture::new(vec![
            Response::http(429, &format!("Retry-After: {header}\r\n"), ""),
            Response::success(),
        ])
        .await;
        fixture
            .provider(config())
            .activate("TEST-KEY")
            .await
            .unwrap();
        let first = fixture.next().await;
        let second = fixture.next().await;
        let tolerance = Duration::from_millis(40);
        if let Some(deadline) = deadline {
            assert!(second.wall_time + tolerance >= deadline);
        } else {
            assert!(second.at.duration_since(first.at) + tolerance >= Duration::from_secs(1));
        }
    }
}

#[tokio::test]
async fn over_limit_retry_after_is_not_shortened_and_invalid_header_uses_backoff() {
    for header in [
        "3",
        "99999999999999999999999999999999999999999999999999999999999999999999999999999999999999",
    ] {
        let fixture = Fixture::new(vec![Response::http(
            429,
            &format!("Retry-After: {header}\r\n"),
            "",
        )])
        .await;
        let error = failure(fixture.provider(config()).activate("TEST-KEY").await);
        assert_eq!(error.code, ErrorCode::RateLimited);
        assert_eq!(
            error.retry_after_seconds,
            Some(header.parse::<u64>().unwrap_or(u64::MAX))
        );
        assert_eq!(
            error.mutation_outcome,
            Some(software_licensing::MutationOutcome::NotApplied)
        );
        fixture.assert_attempts(1).await;
    }
    let mut configuration = config();
    configuration.base_retry_delay = 0.03;
    let mut fixture = Fixture::new(vec![
        Response::http(503, "Retry-After: malformed\r\n", ""),
        Response::http(503, "", ""),
        Response::success_for(Operation::Validate),
    ])
    .await;
    assert!(
        fixture
            .provider(configuration)
            .validate(activation())
            .await
            .unwrap()
            .is_valid
    );
    let first = fixture.next().await;
    let second = fixture.next().await;
    let third = fixture.next().await;
    assert!(second.at.duration_since(first.at) >= Duration::from_millis(28));
    assert!(third.at.duration_since(second.at) >= Duration::from_millis(58));
}

#[tokio::test]
async fn past_http_date_allows_immediate_retry() {
    let fixture = Fixture::new(vec![
        Response::http(429, "Retry-After: Wed, 15 Nov 2023 00:00:00 GMT\r\n", ""),
        Response::success(),
    ])
    .await;
    fixture
        .provider(config())
        .activate("TEST-KEY")
        .await
        .unwrap();
    assert_eq!(fixture.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn header_body_timeouts_and_transport_errors_are_never_replayed() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for response in [
            Response::success().delayed_headers(),
            Response::success().delayed_body(),
            Response::raw(vec![]),
            Response::raw(
                b"HTTP/1.1 200 OK\r\nPolar-Version: 2026-10\r\nContent-Length: 100\r\n\r\n{\"private\":\"TEST-KEY\"}"
                    .to_vec(),
            ),
        ] {
            let fixture = Fixture::new(vec![response]).await;
            let mut configuration = config();
            configuration.request_timeout_seconds = 0.04;
            configuration.operation_timeout_seconds = 0.1;
            let error = failure(
                fixture
                    .provider(configuration)
                    .execute(intent(operation))
                    .await,
            );
            assert_eq!(error.code, ErrorCode::TransportFailure);
            assert!(
                !format!(
                    "{error:?} {error} {}",
                    serde_json::to_string(&error).unwrap()
                )
                .contains("TEST-KEY")
            );
            fixture.assert_attempts(1).await;
        }
    }
}

#[tokio::test]
async fn total_deadline_includes_retry_sleep_and_body_across_attempts() {
    for before_headers in [Duration::ZERO, Duration::from_millis(300)] {
        let mut unavailable = Response::http(503, "", "");
        unavailable.before_headers = before_headers;
        let mut body = Response::success_for(Operation::Validate);
        body.before_body = Duration::from_secs(1);
        let fixture = Fixture::new(vec![unavailable, body]).await;
        let mut configuration = config();
        configuration.request_timeout_seconds = 0.5;
        configuration.operation_timeout_seconds = 0.5;
        configuration.base_retry_delay = 0.3;
        let provider = fixture.provider(configuration);
        let start = Instant::now();
        let error = failure(provider.validate(activation()).await);
        let attempts = fixture.count.load(Ordering::SeqCst);
        // A server-side request count cannot prove that the client received
        // its response before timeout. Both failures are valid with one request.
        assert!(matches!(
            error.code,
            ErrorCode::ServerFailure | ErrorCode::TransportFailure
        ));
        if attempts == 2 {
            assert_eq!(error.code, ErrorCode::TransportFailure);
        }
        // Exclude client construction and leave a scheduling margin on shared
        // runners. Restarting the timeout after retry sleep would take 800 ms;
        // waiting for the complete response body would take still longer.
        assert!(start.elapsed() < Duration::from_millis(700));
        // A slow first response or scheduler delay can leave too little total
        // budget for the retry. Both one and two requests are valid; a retry
        // must never be required merely to satisfy this deadline assertion.
        if before_headers.is_zero() {
            assert!((1..=2).contains(&attempts));
        } else {
            // 300 ms headers + 300 ms retry delay cannot fit a 500 ms operation.
            assert_eq!(attempts, 1);
        }
        fixture.assert_attempts(attempts).await;
    }

    let fixture = Fixture::new(vec![Response::http(429, "Retry-After: 1\r\n", "")]).await;
    let mut configuration = config();
    configuration.request_timeout_seconds = 0.5;
    configuration.operation_timeout_seconds = 0.5;
    let rejected = failure(fixture.provider(configuration).activate("TEST-KEY").await);
    assert_eq!(rejected.code, ErrorCode::RateLimited);
    assert_eq!(rejected.retry_after_seconds, Some(1));
    assert_eq!(
        rejected.mutation_outcome,
        Some(software_licensing::MutationOutcome::NotApplied)
    );
    fixture.assert_attempts(1).await;
}

#[tokio::test]
async fn connection_refusal_is_not_applied_but_lost_response_is_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let provider = PolarLicenseProvider::for_test(config(), endpoint).unwrap();
    let unavailable = failure(provider.activate("TEST-KEY").await);
    assert_eq!(unavailable.code, ErrorCode::TransportFailure);
    assert_eq!(
        unavailable.mutation_outcome,
        Some(software_licensing::MutationOutcome::NotApplied)
    );
    let fixture = Fixture::new(vec![Response::raw(vec![])]).await;
    let lost = failure(fixture.provider(config()).activate("TEST-KEY").await);
    assert_eq!(
        lost.mutation_outcome,
        Some(software_licensing::MutationOutcome::Unknown)
    );
    fixture.assert_attempts(1).await;
}

#[tokio::test]
async fn dropping_future_during_headers_body_or_retry_sleep_has_no_late_attempt() {
    for response in [
        Response::success().delayed_headers(),
        Response::success().delayed_body(),
        Response::http(429, "Retry-After: 1\r\n", ""),
    ] {
        let mut fixture = Fixture::new(vec![response]).await;
        let provider = fixture.provider(config());
        {
            let execution = provider.activate("TEST-KEY");
            tokio::pin!(execution);
            tokio::select! {
                _ = &mut execution => panic!("operation completed before cancellation"),
                _ = fixture.next() => {}
            }
            // Poll while the server sends headers/body or starts the retry wait.
            tokio::select! {
                _ = &mut execution => panic!("operation completed before cancellation"),
                _ = sleep(Duration::from_millis(20)) => {}
            }
        }
        fixture.assert_attempts(1).await;
        sleep(Duration::from_millis(850)).await;
        assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn malformed_rejected_and_mismatched_responses_are_secret_safe() {
    for (status, body, expected) in [
        (
            200,
            b"not JSON TEST-KEY".to_vec(),
            ErrorCode::UnexpectedProviderResponse,
        ),
        (200, vec![0xff], ErrorCode::UnexpectedProviderResponse),
        (200, b"null".to_vec(), ErrorCode::UnexpectedProviderResponse),
        (200, b"{}".to_vec(), ErrorCode::UnexpectedProviderResponse),
        (
            400,
            br#"{"error":"TEST-KEY: activation limit reached"}"#.to_vec(),
            ErrorCode::RequestFailure,
        ),
        (404, b"TEST-KEY".to_vec(), ErrorCode::RequestFailure),
        (401, b"TEST-KEY".to_vec(), ErrorCode::RequestFailure),
    ] {
        let fixture = Fixture::new(vec![Response::http(status, "", body)]).await;
        let error = failure(fixture.provider(config()).activate("TEST-KEY").await);
        assert_eq!(error.code, expected);
        assert!(!serde_json::to_string(&error).unwrap().contains("TEST-KEY"));
    }
    for operation in [Operation::Activate, Operation::Validate] {
        for field in ["organization_id", "benefit_id", "key"] {
            let mut value = valid_for(operation);
            let key = if operation == Operation::Activate {
                &mut value["license_key"]
            } else {
                &mut value
            };
            key[field] = json!(if field == "key" { "WRONG-KEY" } else { OTHER });
            let fixture = Fixture::new(vec![Response::http(200, "", value.to_string())]).await;
            let error = failure(fixture.provider(config()).execute(intent(operation)).await);
            assert_eq!(
                error.code,
                if operation == Operation::Validate {
                    ErrorCode::UnexpectedProviderResponse
                } else {
                    ErrorCode::InvalidLicense
                },
                "{operation:?} {field}"
            );
            assert!(!serde_json::to_string(&error).unwrap().contains("TEST-KEY"));
        }
    }
}

#[tokio::test]
async fn execute_returns_provider_rejections_as_errors_for_every_operation() {
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        let fixture = Fixture::new(vec![Response::http(
            400,
            "",
            r#"{"error":"TEST-KEY: activation limit reached"}"#,
        )])
        .await;
        assert_eq!(
            failure(fixture.provider(config()).execute(intent(operation)).await).code,
            ErrorCode::RequestFailure,
        );
    }
}

#[tokio::test]
async fn expired_success_retains_created_identity_and_bad_input_never_sends() {
    let mut value = valid();
    value["license_key"]["expires_at"] = json!("2023-11-14T22:13:25Z");
    let fixture = Fixture::new(vec![Response::http(200, "", value.to_string())]).await;
    let provider = fixture.provider(config());
    let activation = provider.activate("TEST-KEY").await.unwrap();
    assert_eq!(activation.source, "polar");
    assert_eq!(
        activation.activation_identifier.as_deref(),
        Some(ACTIVATION)
    );
    assert_eq!(activation.expires_at, Some(1700000005.0));
    assert_eq!(
        failure(provider.activate(" ").await).code,
        ErrorCode::InvalidLicenseKey
    );
    let mut wrong_source = activation;
    wrong_source.source = "other".into();
    assert_eq!(
        failure(provider.validate(wrong_source).await).code,
        ErrorCode::InvalidLicense
    );
    assert_eq!(fixture.count.load(Ordering::SeqCst), 1);
}

fn unconfirmed_version_fields() -> [String; 6] {
    let matching = format!("Polar-Version: {API_VERSION}\r\n");
    let different = "Polar-Version: 2025-01\r\n";
    [
        String::new(),
        different.into(),
        format!("{matching}{different}"),
        format!("{different}{matching}"),
        format!("{matching}{matching}"),
        format!("Polar-Version: {API_VERSION}, 2025-01\r\n"),
    ]
}

#[tokio::test]
async fn unconfirmed_api_version_never_accepts_success_or_revokes_a_license() {
    for version in unconfirmed_version_fields() {
        for operation in [
            Operation::Activate,
            Operation::Validate,
            Operation::Deactivate,
        ] {
            let body = if operation == Operation::Deactivate {
                String::new()
            } else {
                valid_for(operation).to_string()
            };
            let status = if operation == Operation::Deactivate {
                204
            } else {
                200
            };
            let bytes=format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\n{version}Connection: close\r\n\r\n{body}",body.len()).into_bytes();
            let fixture = Fixture::new(vec![Response::raw(bytes)]).await;
            let error = failure(fixture.provider(config()).execute(intent(operation)).await);
            assert_eq!(error.code, ErrorCode::UnexpectedProviderResponse);
            assert_eq!(
                error.mutation_outcome,
                (operation != Operation::Validate)
                    .then_some(software_licensing::MutationOutcome::Unknown)
            );
            fixture.assert_attempts(1).await;
        }
        let body = r#"{"error":"ResourceNotFound","detail":"TEST-KEY"}"#;
        let bytes=format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n{version}Connection: close\r\n\r\n{body}",body.len()).into_bytes();
        let fixture = Fixture::new(vec![Response::raw(bytes)]).await;
        let error = failure(fixture.provider(config()).validate(activation()).await);
        assert_eq!(error.code, ErrorCode::RequestFailure);
        assert_eq!(error.status_code, Some(404));
        assert!(!serde_json::to_string(&error).unwrap().contains("TEST-KEY"));
    }
}

#[tokio::test]
async fn unconfirmed_client_errors_do_not_authorize_mutation_replays() {
    let versions = unconfirmed_version_fields();
    for operation in [
        Operation::Activate,
        Operation::Validate,
        Operation::Deactivate,
    ] {
        for (status, code) in [
            (404, ErrorCode::RequestFailure),
            (429, ErrorCode::RateLimited),
        ] {
            for version in &versions {
                let response = Response::raw(
                    format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\n{version}Retry-After: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                );
                let fixture = Fixture::new(vec![response, Response::success_for(operation)]).await;
                let result = fixture.provider(config()).execute(intent(operation)).await;
                if operation == Operation::Validate && status == 429 {
                    assert!(matches!(result, Ok(ProviderReply::Validated { .. })));
                    fixture.assert_attempts(2).await;
                    continue;
                }
                assert_eq!(
                    failure(result),
                    Failure {
                        code,
                        status_code: Some(status),
                        retry_after_seconds: Some(0),
                        mutation_outcome: (operation != Operation::Validate)
                            .then_some(MutationOutcome::Unknown),
                    },
                    "{operation:?}/{status}/{version:?}"
                );
                fixture.assert_attempts(1).await;
            }
        }
    }
}

#[tokio::test]
async fn typed_rejections_require_api_version_and_never_report_deactivation_success() {
    for (status, error_name) in [(403, "NotPermitted"), (404, "ResourceNotFound")] {
        for operation in [
            Operation::Activate,
            Operation::Validate,
            Operation::Deactivate,
        ] {
            let body = json!({"error":error_name,"detail":"TEST-KEY"});
            let fixture = Fixture::new(vec![Response::http(status, "", body.to_string())]).await;
            let error = failure(fixture.provider(config()).execute(intent(operation)).await);
            assert_eq!(error.code, ErrorCode::InvalidLicense);
            assert_eq!(error.status_code, Some(status));
            assert_eq!(
                error.mutation_outcome,
                (operation != Operation::Validate)
                    .then_some(software_licensing::MutationOutcome::NotApplied)
            );
            assert!(!serde_json::to_string(&error).unwrap().contains("TEST-KEY"));
            fixture.assert_attempts(1).await;
        }
    }
}

#[tokio::test]
async fn saved_activation_wire_rejections_precede_transport() {
    let mut fixture = Fixture::new(vec![
        Response::success_for(Operation::Validate),
        Response::success_for(Operation::Deactivate),
    ])
    .await;
    let provider = fixture.provider(config());
    for (field, value, code) in [
        ("source", json!("other-provider"), ErrorCode::InvalidLicense),
        ("source", json!("POLAR"), ErrorCode::InvalidLicense),
        ("planIdentifier", json!(OTHER), ErrorCode::InvalidLicense),
        (
            "licenseKey",
            Value::Null,
            ErrorCode::InvalidProviderConfiguration,
        ),
        (
            "licenseKey",
            json!(" \t\n"),
            ErrorCode::InvalidProviderConfiguration,
        ),
        (
            "licenseKey",
            json!("x".repeat(4097)),
            ErrorCode::InvalidData,
        ),
        (
            "activationIdentifier",
            Value::Null,
            ErrorCode::InvalidProviderConfiguration,
        ),
        (
            "activationIdentifier",
            json!("not-a-uuid"),
            ErrorCode::InvalidProviderConfiguration,
        ),
    ] {
        let mut saved = serde_json::to_value(activation()).unwrap();
        saved[field] = value;
        for operation in [Operation::Validate, Operation::Deactivate] {
            let payload = json!({"operation": operation, "activation": saved});
            let intent: Intent =
                software_licensing::decode(&serde_json::to_vec(&payload).unwrap()).unwrap();
            assert_eq!(
                failure(provider.execute(intent).await),
                Failure {
                    mutation_outcome: (operation == Operation::Deactivate)
                        .then_some(MutationOutcome::NotApplied),
                    ..Failure::new(code)
                },
                "{operation:?} {field}"
            );
        }
    }
    fixture.assert_attempts(0).await;

    provider.validate(activation()).await.unwrap();
    assert!(
        fixture
            .next()
            .await
            .request
            .starts_with("POST /v1/customer-portal/license-keys/validate ")
    );
    provider.deactivate(activation()).await.unwrap();
    assert!(
        fixture
            .next()
            .await
            .request
            .starts_with("POST /v1/customer-portal/license-keys/deactivate ")
    );
    fixture.assert_attempts(2).await;
}

#[tokio::test]
async fn unusable_wire_keys_are_rejected_before_any_transport() {
    let fixture = Fixture::new(vec![]).await;
    let provider = fixture.provider(config());
    let expected = Failure {
        mutation_outcome: Some(MutationOutcome::NotApplied),
        ..Failure::new(ErrorCode::InvalidLicenseKey)
    };
    for (description, key) in [
        ("omitted", None),
        ("null", Some(Value::Null)),
        ("empty", Some(json!(""))),
        ("whitespace", Some(json!(" \t\r\n"))),
        ("oversized", Some(json!("x".repeat(4097)))),
    ] {
        let mut payload = json!({"operation": "activate"});
        if let Some(key) = key {
            payload["licenseKey"] = key;
        }
        let bytes = serde_json::to_vec(&payload).unwrap();
        let intent: Intent = software_licensing::decode(&bytes).unwrap();
        assert_eq!(
            failure(provider.execute(intent).await),
            expected,
            "{description}"
        );
    }
    fixture.assert_attempts(0).await;
}
