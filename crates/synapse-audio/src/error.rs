use synapse_core::error_codes;
use thiserror::Error;

pub type AudioResult<T> = Result<T, AudioError>;

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum AudioError {
    #[error("audio device lost: {detail}")]
    DeviceLost { detail: String },
    #[error("audio loopback init failed: {detail}")]
    LoopbackInitFailed { detail: String },
    #[error("audio STT model not loaded: {detail}")]
    SttModelNotLoaded { detail: String },
}

impl AudioError {
    #[must_use]
    #[tracing::instrument(skip_all, fields(audio_error = ?self))]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DeviceLost { .. } => error_codes::AUDIO_DEVICE_LOST,
            Self::LoopbackInitFailed { .. } => error_codes::AUDIO_LOOPBACK_INIT_FAILED,
            Self::SttModelNotLoaded { .. } => error_codes::AUDIO_STT_MODEL_NOT_LOADED,
        }
    }
}
