use crate::{
    Intent, PolarLicenseConfig,
    protocol::{self, Decision},
};
use reqwest::{Client, ClientBuilder, header::RETRY_AFTER};
use software_licensing::{
    Activation, ErrorCode, Failure, MutationOutcome, Operation, ProviderReply, Validation,
};
use std::{
    future::{Future, pending},
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{Instant, sleep_until, timeout_at};

/// Reusable asynchronous provider with fixed HTTPS License API endpoints.
///
/// This type owns no runtime and starts no per-operation tasks. Each execution
/// runs inside its caller's future. Dropping that future cancels local requests
/// and sleeps; the server might already have applied an activation/deactivation.
#[derive(Clone)]
pub struct PolarLicenseProvider {
    configuration: PolarLicenseConfig,
    client: Client,
    #[cfg(test)]
    test_endpoint: Option<String>,
    #[cfg(test)]
    pub(crate) test_wall_time: Option<SystemTime>,
    #[cfg(test)]
    pub(crate) retry_wait: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl PolarLicenseProvider {
    /// Validates configuration and builds a client without making a request.
    pub fn new(configuration: PolarLicenseConfig) -> Result<Self, Failure> {
        configuration.validated()?;
        let client = client_builder(&configuration)
            .https_only(true)
            .build()
            .map_err(|_| transport_failure())?;
        Ok(Self {
            configuration,
            client,
            #[cfg(test)]
            test_endpoint: None,
            #[cfg(test)]
            test_wall_time: None,
            #[cfg(test)]
            retry_wait: None,
        })
    }

    /// Executes an intent within one total deadline, including body reads and
    /// retries. Transport errors are never replayed, including for validation.
    /// All failures, including provider rejections, use the `Err` channel.
    pub async fn execute(&self, intent: Intent) -> Result<ProviderReply, Failure> {
        self.execute_with_cancellation(intent, pending()).await
    }

    /// Stops local work when `cancelled` completes. A rejection already received
    /// wins cancellation during its retry wait; its status and retry advice survive.
    /// An interrupted unconfirmed mutation remains unknown. Dropping the returned
    /// future stops work without returning any outcome evidence.
    pub async fn execute_with_cancellation(
        &self,
        intent: Intent,
        cancelled: impl Future<Output = ()>,
    ) -> Result<ProviderReply, Failure> {
        let operation = intent.operation;
        let deadline =
            Instant::now() + Duration::from_secs_f64(self.configuration.operation_timeout_seconds);
        // Each await has the total deadline applied below. An outer timeout would
        // discard a known rate-limit rejection while sleeping, or a confirmed
        // mutation response just received at its deadline.
        self.execute_until(intent, deadline, cancelled)
            .await
            .map_err(|mut failure| {
                if operation == Operation::Validate {
                    failure.mutation_outcome = None;
                } else if failure.mutation_outcome.is_none() {
                    failure.mutation_outcome = Some(failure.mutation_outcome());
                }
                failure
            })
    }

    pub async fn activate(&self, license_key: impl Into<String>) -> Result<Activation, Failure> {
        match self
            .execute(Intent {
                operation: Operation::Activate,
                license_key: Some(license_key.into()),
                activation: None,
            })
            .await?
        {
            ProviderReply::Activated { activation } => Ok(activation),
            _ => Err(unexpected_response()),
        }
    }

    pub async fn validate(&self, activation: Activation) -> Result<Validation, Failure> {
        match self
            .execute(Intent {
                operation: Operation::Validate,
                license_key: None,
                activation: Some(activation),
            })
            .await?
        {
            ProviderReply::Validated { validation } => Ok(validation),
            _ => Err(unexpected_response()),
        }
    }

    pub async fn deactivate(&self, activation: Activation) -> Result<(), Failure> {
        match self
            .execute(Intent {
                operation: Operation::Deactivate,
                license_key: None,
                activation: Some(activation),
            })
            .await?
        {
            ProviderReply::Deactivated => Ok(()),
            _ => Err(unexpected_response()),
        }
    }

    async fn execute_until(
        &self,
        intent: Intent,
        deadline: Instant,
        cancelled: impl Future<Output = ()>,
    ) -> Result<ProviderReply, Failure> {
        tokio::pin!(cancelled);
        let prepared = protocol::request(&self.configuration, intent)?;
        let url = prepared.url.as_str();
        #[cfg(test)]
        let test_url = self
            .test_endpoint
            .as_ref()
            .map(|base| format!("{base}/{}", url.rsplit('/').next().unwrap()));
        #[cfg(test)]
        let url = test_url.as_deref().unwrap_or(url);

        let mut last_rejection = None;
        for attempt in 1..=self.configuration.maximum_request_attempts {
            // timeout_at can poll its inner future once at an elapsed deadline.
            // Check before sending so an expired retry never starts extra work.
            if Instant::now() >= deadline {
                return Err(last_rejection.unwrap_or_else(|| {
                    let mut failure = transport_failure();
                    failure.mutation_outcome = Some(MutationOutcome::NotApplied);
                    failure
                }));
            }
            let request_deadline = deadline.min(
                Instant::now()
                    + Duration::from_secs_f64(self.configuration.request_timeout_seconds),
            );
            let response = self
                .send(
                    url,
                    &prepared,
                    request_deadline,
                    cancelled.as_mut(),
                    last_rejection.as_ref(),
                )
                .await?;
            let now = SystemTime::now();
            #[cfg(test)]
            let now = self.test_wall_time.unwrap_or(now);
            let (delay, rejection) = match protocol::respond(
                &self.configuration,
                &prepared,
                response,
                attempt,
                utc_time(now),
            ) {
                Decision::Complete(result) => return result,
                Decision::Retry { delay, rejection } => (delay, rejection),
            };
            let wake = Instant::now() + Duration::from_secs_f64(delay);
            if wake >= deadline {
                return Err(rejection);
            }
            #[cfg(test)]
            if let Some(observer) = &self.retry_wait {
                observer.notify_one();
            }
            tokio::select! {
                biased;
                _ = &mut cancelled => return Err(rejection),
                _ = sleep_until(wake) => {},
            }
            last_rejection = Some(rejection);
        }
        Err(unexpected_response())
    }

    async fn send(
        &self,
        url: &str,
        prepared: &protocol::HttpRequest,
        deadline: Instant,
        mut cancelled: Pin<&mut impl Future<Output = ()>>,
        last_rejection: Option<&Failure>,
    ) -> Result<protocol::HttpResponse, Failure> {
        let mut request = self.client.post(url).body(prepared.body.clone());
        for (name, value) in protocol::REQUEST_HEADERS {
            request = request.header(name, value);
        }
        let entered = AtomicBool::new(false);
        let sending = async {
            entered.store(true, Ordering::Relaxed);
            timeout_at(deadline, request.send())
                .await
                .map_err(|_| transport_failure())?
                .map_err(|error| {
                    let mut failure = transport_failure();
                    if error.is_connect() {
                        failure.mutation_outcome = Some(MutationOutcome::NotApplied);
                    }
                    failure
                })
        };
        let response = tokio::select! {
            biased;
            _ = cancelled.as_mut() => return Err(if entered.load(Ordering::Relaxed) {
                cancelled_failure(MutationOutcome::Unknown)
            } else {
                last_rejection.cloned().unwrap_or_else(|| cancelled_failure(MutationOutcome::NotApplied))
            }),
            response = sending => response?,
        };
        let status_code = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let versions = response.headers().get_all("Polar-Version");
        let mut versions = versions.iter();
        let api_version = match (versions.next(), versions.next()) {
            (Some(version), None) => version.to_str().ok().map(str::to_owned),
            _ => None,
        };
        // These statuses already determine the stable rejection. Do not lose
        // known retry/outcome information while waiting for an irrelevant body.
        if status_code == 429 || status_code >= 500 {
            return Ok(protocol::HttpResponse {
                status_code,
                body: String::new(),
                retry_after,
                api_version,
            });
        }
        let body = tokio::select! {
            biased;
            _ = cancelled.as_mut() => Err(cancelled_failure(MutationOutcome::Unknown)),
            result = timeout_at(deadline, response_body(response)) => result.unwrap_or_else(|_| Err(transport_failure())),
        };
        // Retain the status and version even if the body cannot refine a 4xx.
        let body = match body {
            Ok(body) => body,
            Err(_) if (400..500).contains(&status_code) => String::new(),
            Err(failure) => return Err(failure),
        };
        Ok(protocol::HttpResponse {
            status_code,
            body,
            retry_after,
            api_version,
        })
    }

    // Only unit tests can choose an owned loopback fixture or bypass proxies.
    #[cfg(test)]
    pub(crate) fn for_test(
        configuration: PolarLicenseConfig,
        endpoint: String,
    ) -> Result<Self, Failure> {
        configuration.validated()?;
        let client = client_builder(&configuration)
            .no_proxy()
            .build()
            .map_err(|_| transport_failure())?;
        Ok(Self {
            configuration,
            client,
            test_endpoint: Some(endpoint),
            test_wall_time: None,
            retry_wait: None,
        })
    }
}

async fn response_body(mut response: reqwest::Response) -> Result<String, Failure> {
    if response
        .content_length()
        .is_some_and(|n| n > protocol::MAX_RESPONSE_BYTES as u64)
    {
        return Err(unexpected_response());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| transport_failure())? {
        if chunk.len() > protocol::MAX_RESPONSE_BYTES - bytes.len() {
            return Err(unexpected_response());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| unexpected_response())
}

fn cancelled_failure(outcome: MutationOutcome) -> Failure {
    let mut failure = Failure::new(ErrorCode::Cancelled);
    failure.mutation_outcome = Some(outcome);
    failure
}

fn client_builder(configuration: &PolarLicenseConfig) -> ClientBuilder {
    Client::builder()
        .tls_backend_native()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .connection_verbose(false)
        .timeout(Duration::from_secs_f64(
            configuration.request_timeout_seconds,
        ))
    // No cookie store, alternate roots, insecure TLS or request/body logging.
    // Production retains reqwest's normal OS certificates and system proxies.
}

fn utc_time(now: SystemTime) -> f64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    }
}
fn transport_failure() -> Failure {
    Failure::new(ErrorCode::TransportFailure)
}
fn unexpected_response() -> Failure {
    Failure::new(ErrorCode::UnexpectedProviderResponse)
}
