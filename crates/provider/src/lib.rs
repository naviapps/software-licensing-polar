//! Polar License API provider. HTTP and retry decisions stay in Rust.
//!
//! Construct with [`PolarLicenseProvider::new`], then await
//! [`PolarLicenseProvider::execute`] on the caller's Tokio runtime.
//! Dropping that future stops local work, but cannot undo server-side effects.
mod protocol;
mod transport;

pub use protocol::{API_VERSION, Environment, Intent, LicenseScope, PolarLicenseConfig};
pub use transport::PolarLicenseProvider;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod transport_tests;
