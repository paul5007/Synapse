use thiserror::Error;

/// Telemetry initialization failures.
#[derive(Debug, Error)]
pub enum TelemetryError {
    /// A global tracing subscriber was already configured.
    #[error("tracing subscriber was already initialized")]
    AlreadyInitialized,
}

/// Initialize the process-wide tracing subscriber.
///
/// # Errors
///
/// Returns [`TelemetryError::AlreadyInitialized`] when another caller has
/// already installed a global tracing subscriber.
pub fn init() -> Result<(), TelemetryError> {
    let subscriber = tracing_subscriber::fmt().json().finish();
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| TelemetryError::AlreadyInitialized)
}
