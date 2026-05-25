pub mod error;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use error::{AudioError, AudioResult};

pub const DEFAULT_RING_SECONDS: u32 = 5;
pub const MAX_RING_SECONDS: u32 = 5;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioConfig {
    #[serde(default = "default_ring_seconds")]
    pub ring_seconds: u32,
    #[serde(default)]
    pub start_loopback: bool,
    #[serde(default)]
    pub detectors_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model_path: Option<PathBuf>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            ring_seconds: DEFAULT_RING_SECONDS,
            start_loopback: false,
            detectors_enabled: false,
            stt_model_path: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioRuntime {
    config: AudioConfig,
    loopback_started: bool,
    detectors_started: bool,
}

impl AudioRuntime {
    /// Spawns the M3 audio runtime scaffold.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::LoopbackInitFailed`] when the ring buffer duration
    /// is outside the scaffold's supported range or when the caller requests
    /// loopback/detector startup before the dedicated loopback implementation is
    /// available.
    #[tracing::instrument(skip_all, fields(component = "audio_runtime"))]
    pub fn spawn(config: AudioConfig) -> AudioResult<Self> {
        validate_config(&config)?;
        Ok(Self {
            config,
            loopback_started: false,
            detectors_started: false,
        })
    }

    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "audio_runtime"))]
    pub fn config(&self) -> &AudioConfig {
        &self.config
    }

    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "audio_runtime"))]
    pub fn loopback_started(&self) -> bool {
        self.loopback_started
    }

    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "audio_runtime"))]
    pub fn detectors_started(&self) -> bool {
        self.detectors_started
    }
}

fn validate_config(config: &AudioConfig) -> AudioResult<()> {
    if config.ring_seconds == 0 || config.ring_seconds > MAX_RING_SECONDS {
        return Err(AudioError::LoopbackInitFailed {
            detail: format!(
                "audio ring_seconds must be between 1 and {MAX_RING_SECONDS}, got {}",
                config.ring_seconds
            ),
        });
    }
    if config.start_loopback {
        return Err(AudioError::LoopbackInitFailed {
            detail: "audio loopback startup is not available in the scaffold".to_owned(),
        });
    }
    if config.detectors_enabled {
        return Err(AudioError::LoopbackInitFailed {
            detail: "audio detectors require loopback startup".to_owned(),
        });
    }
    Ok(())
}

const fn default_ring_seconds() -> u32 {
    DEFAULT_RING_SECONDS
}

#[cfg(test)]
mod tests {
    use synapse_core::error_codes;

    use super::{AudioConfig, AudioError, AudioRuntime, DEFAULT_RING_SECONDS, MAX_RING_SECONDS};

    #[test]
    fn default_spawn_keeps_audio_paths_stopped() -> Result<(), AudioError> {
        let runtime = AudioRuntime::spawn(AudioConfig::default())?;

        assert_eq!(runtime.config().ring_seconds, DEFAULT_RING_SECONDS);
        assert!(!runtime.loopback_started());
        assert!(!runtime.detectors_started());
        Ok(())
    }

    #[test]
    fn invalid_ring_seconds_fail_closed() {
        let zero = spawn_error(AudioConfig {
            ring_seconds: 0,
            ..AudioConfig::default()
        });
        assert_eq!(zero.code(), error_codes::AUDIO_LOOPBACK_INIT_FAILED);

        let too_large = spawn_error(AudioConfig {
            ring_seconds: MAX_RING_SECONDS.saturating_add(1),
            ..AudioConfig::default()
        });
        assert_eq!(too_large.code(), error_codes::AUDIO_LOOPBACK_INIT_FAILED);
    }

    #[test]
    fn unavailable_loopback_and_detectors_fail_closed() {
        let loopback = spawn_error(AudioConfig {
            start_loopback: true,
            ..AudioConfig::default()
        });
        assert_eq!(loopback.code(), error_codes::AUDIO_LOOPBACK_INIT_FAILED);

        let detectors = spawn_error(AudioConfig {
            detectors_enabled: true,
            ..AudioConfig::default()
        });
        assert_eq!(detectors.code(), error_codes::AUDIO_LOOPBACK_INIT_FAILED);
    }

    fn spawn_error(config: AudioConfig) -> AudioError {
        match AudioRuntime::spawn(config) {
            Ok(runtime) => panic!("expected AudioRuntime::spawn to fail, got {runtime:?}"),
            Err(error) => error,
        }
    }
}
