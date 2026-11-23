//! Owned asynchronous C boundary. No host callback runs under a Rust lock.
use software_licensing::{ErrorCode, Failure};
use software_licensing_polar::{Intent, PolarLicenseConfig, PolarLicenseProvider};
use std::ffi::c_void;
use std::sync::{Arc, OnceLock};
use tokio::{runtime::Runtime, sync::watch};
use zeroize::Zeroizing;

pub struct SoftwareLicensingPolarProvider(Arc<PolarLicenseProvider>);
pub struct SoftwareLicensingPolarOperation(watch::Sender<bool>);
pub struct SoftwareLicensingPolarResult {
    code: u32,
    bytes: Zeroizing<Vec<u8>>,
}
#[repr(C)]
pub struct SoftwareLicensingPolarResultView {
    pub code: u32,
    pub data: *const u8,
    pub length: usize,
}
type SoftwareLicensingPolarCompletion =
    extern "C" fn(*mut c_void, *mut SoftwareLicensingPolarResult);

impl SoftwareLicensingPolarResult {
    fn result<T: serde::Serialize>(value: Result<T, Failure>) -> *mut Self {
        let (code, bytes) = match value {
            Ok(value) => match serde_json::to_vec(&value) {
                Ok(bytes) => (0, bytes),
                Err(_) => (1, br#"{"code":"unexpectedProviderResponse"}"#.to_vec()),
            },
            Err(failure) => (1, serde_json::to_vec(&failure).expect("stable error code")),
        };
        Box::into_raw(Box::new(Self {
            code,
            bytes: Zeroizing::new(bytes),
        }))
    }
}

fn invalid() -> Failure {
    Failure::new(ErrorCode::InvalidData)
}
unsafe fn input<'a>(data: *const u8, length: usize) -> Result<&'a [u8], Failure> {
    if data.is_null() || length == 0 || length > software_licensing::MAX_BYTES {
        return Err(invalid());
    }
    Ok(unsafe { std::slice::from_raw_parts(data, length) })
}
fn runtime() -> Result<&'static Runtime, Failure> {
    static RUNTIME: OnceLock<Result<Runtime, Failure>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("polar")
                .enable_all()
                .build()
                .map_err(|_| Failure::new(ErrorCode::TransportFailure))
        })
        .as_ref()
        .map_err(Clone::clone)
}

struct Completion {
    callback: Option<SoftwareLicensingPolarCompletion>,
    context: usize,
}
impl Completion {
    fn finish<T: serde::Serialize>(&mut self, result: Result<T, Failure>) {
        if let Some(callback) = self.callback.take() {
            callback(
                self.context as *mut c_void,
                SoftwareLicensingPolarResult::result(result),
            );
        }
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.finish::<()>(Err(Failure::new(ErrorCode::Cancelled)));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn software_licensing_polar_abi_version() -> u32 {
    3
}

/// # Safety
/// Input is readable for length bytes. Error is null or writable for one owned result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_provider_create(
    data: *const u8,
    length: usize,
    error: *mut *mut SoftwareLicensingPolarResult,
) -> *mut SoftwareLicensingPolarProvider {
    if !error.is_null() {
        unsafe {
            *error = std::ptr::null_mut();
        }
    }
    let result = (|| {
        let configuration: PolarLicenseConfig =
            software_licensing::decode(unsafe { input(data, length)? })
                .map_err(|_| Failure::new(ErrorCode::InvalidProviderConfiguration))?;
        let runtime = runtime()?;
        let _entered = runtime.enter();
        PolarLicenseProvider::new(configuration)
    })();
    match result {
        Ok(provider) => Box::into_raw(Box::new(SoftwareLicensingPolarProvider(Arc::new(provider)))),
        Err(failure) => {
            if !error.is_null() {
                unsafe {
                    *error = SoftwareLicensingPolarResult::result::<()>(Err(failure));
                }
            }
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// Provider is null or exclusively owned with no concurrent start calls remaining.
/// In-flight operations retain their own provider; their completion remains mandatory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_provider_destroy(
    provider: *mut SoftwareLicensingPolarProvider,
) {
    if !provider.is_null() {
        drop(unsafe { Box::from_raw(provider) });
    }
}

/// # Safety
/// Provider stays live through this call; input is readable for length bytes.
/// Callback must not unwind and owns its result. Context stays valid until callback.
/// A non-null return completes exactly once, possibly before this call returns.
/// Null provider/callback returns null and produces no callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_operation_start(
    provider: *const SoftwareLicensingPolarProvider,
    data: *const u8,
    length: usize,
    context: *mut c_void,
    callback: Option<SoftwareLicensingPolarCompletion>,
) -> *mut SoftwareLicensingPolarOperation {
    let Some(provider) = (unsafe { provider.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let Some(callback) = callback else {
        return std::ptr::null_mut();
    };
    let intent = unsafe { input(data, length) }.and_then(software_licensing::decode::<Intent>);
    let provider = Arc::clone(&provider.0);
    let (sender, mut cancelled) = watch::channel(false);
    let operation = Box::into_raw(Box::new(SoftwareLicensingPolarOperation(sender)));
    let mut completion = Completion {
        callback: Some(callback),
        context: context as usize,
    };
    match runtime() {
        Ok(runtime) => {
            runtime.spawn(async move {
                let reply = match intent {
                    Err(error) => Err(error),
                    Ok(intent) => {
                        provider
                            .execute_with_cancellation(intent, async {
                                let _ = cancelled.changed().await;
                            })
                            .await
                    }
                };
                completion.finish(reply);
            });
        }
        Err(error) => completion.finish::<()>(Err(error)),
    }
    operation
}

/// # Safety
/// Operation is null or live. Cancellation cannot undo server-side effects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_operation_cancel(
    operation: *const SoftwareLicensingPolarOperation,
) {
    if let Some(operation) = unsafe { operation.as_ref() } {
        operation.0.send_replace(true);
    }
}

/// # Safety
/// Operation is null or exclusively owned with no concurrent cancel calls remaining.
/// Destruction cancels local work; context must still live until its one completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_operation_destroy(
    operation: *mut SoftwareLicensingPolarOperation,
) {
    if !operation.is_null() {
        let operation = unsafe { Box::from_raw(operation) };
        operation.0.send_replace(true);
    }
}

/// # Safety
/// Owner is null or live; views are borrowed until owner is destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_result_view(
    owner: *const SoftwareLicensingPolarResult,
) -> SoftwareLicensingPolarResultView {
    match unsafe { owner.as_ref() } {
        Some(owner) => SoftwareLicensingPolarResultView {
            code: owner.code,
            data: owner.bytes.as_ptr(),
            length: owner.bytes.len(),
        },
        None => SoftwareLicensingPolarResultView {
            code: 1,
            data: std::ptr::null(),
            length: 0,
        },
    }
}

/// # Safety
/// Owner is null or exclusively owned; all borrowed views expire on destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn software_licensing_polar_result_destroy(
    owner: *mut SoftwareLicensingPolarResult,
) {
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;

    const CONFIG: &[u8] =
        br#"{"activationInstanceName":"Fixture","environment":"sandbox","licenseScope":{"organizationId":"11111111-1111-4111-8111-111111111111","benefitIds":["22222222-2222-4222-8222-222222222222"]}}"#;
    const EMPTY_KEY: &[u8] = br#"{"operation":"activate","licenseKey":""}"#;

    struct Capture {
        sender: mpsc::Sender<(u32, Vec<u8>)>,
        calls: Arc<AtomicUsize>,
    }
    extern "C" fn completed(context: *mut c_void, result: *mut SoftwareLicensingPolarResult) {
        let capture = unsafe { Box::from_raw(context.cast::<Capture>()) };
        let view = unsafe { software_licensing_polar_result_view(result) };
        let bytes = if view.data.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec()
        };
        capture.calls.fetch_add(1, Ordering::SeqCst);
        unsafe {
            software_licensing_polar_result_destroy(result);
        }
        let _ = capture.sender.send((view.code, bytes));
    }
    fn provider() -> *mut SoftwareLicensingPolarProvider {
        let mut error = std::ptr::null_mut();
        let provider = unsafe {
            software_licensing_polar_provider_create(CONFIG.as_ptr(), CONFIG.len(), &mut error)
        };
        assert!(error.is_null() && !provider.is_null());
        provider
    }

    #[test]
    fn configuration_failures_own_independent_bounded_results() {
        for (data, length, expected) in [
            (std::ptr::null(), 0, ErrorCode::InvalidData),
            (CONFIG.as_ptr(), usize::MAX, ErrorCode::InvalidData),
            (b"{".as_ptr(), 1, ErrorCode::InvalidProviderConfiguration),
        ] {
            let mut error = std::ptr::null_mut();
            let owner =
                unsafe { software_licensing_polar_provider_create(data, length, &mut error) };
            assert!(owner.is_null() && !error.is_null());
            let view = unsafe { software_licensing_polar_result_view(error) };
            assert_eq!(view.code, 1);
            let saved = unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec();
            unsafe {
                software_licensing_polar_result_destroy(error);
            }
            assert_eq!(
                serde_json::from_slice::<Failure>(&saved).unwrap().code,
                expected
            );
        }
        let empty = unsafe { software_licensing_polar_result_view(std::ptr::null()) };
        assert!(empty.data.is_null() && empty.length == 0 && empty.code == 1);
    }

    #[test]
    fn null_start_contract_produces_no_operation() {
        let owner = provider();
        unsafe {
            assert!(
                software_licensing_polar_operation_start(
                    owner,
                    EMPTY_KEY.as_ptr(),
                    EMPTY_KEY.len(),
                    std::ptr::null_mut(),
                    None
                )
                .is_null()
            );
            assert!(
                software_licensing_polar_operation_start(
                    std::ptr::null(),
                    EMPTY_KEY.as_ptr(),
                    EMPTY_KEY.len(),
                    std::ptr::null_mut(),
                    Some(completed)
                )
                .is_null()
            );
            software_licensing_polar_provider_destroy(owner);
            software_licensing_polar_provider_destroy(std::ptr::null_mut());
            software_licensing_polar_operation_cancel(std::ptr::null());
            software_licensing_polar_operation_destroy(std::ptr::null_mut());
            software_licensing_polar_result_destroy(std::ptr::null_mut());
        }
    }

    #[test]
    fn callbacks_own_one_result_when_provider_or_operation_is_released_early() {
        for destroy_early in [false, true] {
            for _ in 0..100 {
                let owner = provider();
                let (sender, receiver) = mpsc::channel();
                let calls = Arc::new(AtomicUsize::new(0));
                let context = Box::into_raw(Box::new(Capture {
                    sender,
                    calls: Arc::clone(&calls),
                }))
                .cast();
                let operation = unsafe {
                    software_licensing_polar_operation_start(
                        owner,
                        EMPTY_KEY.as_ptr(),
                        EMPTY_KEY.len(),
                        context,
                        Some(completed),
                    )
                };
                assert!(!operation.is_null());
                unsafe {
                    software_licensing_polar_provider_destroy(owner);
                }
                if destroy_early {
                    unsafe {
                        software_licensing_polar_operation_destroy(operation);
                    }
                }
                let (code, bytes) = receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("one completion");
                assert_eq!(code, 1);
                let failure: Failure = serde_json::from_slice(&bytes).unwrap();
                assert!(matches!(
                    failure.code,
                    ErrorCode::InvalidLicenseKey | ErrorCode::Cancelled
                ));
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                assert!(receiver.recv_timeout(Duration::from_millis(1)).is_err());
                if !destroy_early {
                    unsafe {
                        software_licensing_polar_operation_destroy(operation);
                    }
                }
            }
        }
    }
}
