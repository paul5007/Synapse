use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use synapse_core::Backend;

const DEFAULT_HOLD_MS: u32 = 33;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActPressParams {
    pub keys: Vec<String>,
    #[serde(default = "default_hold_ms")]
    #[schemars(default = "default_hold_ms", range(min = 1, max = 30000))]
    pub hold_ms: u32,
    #[serde(default = "default_press_backend")]
    #[schemars(default = "default_press_backend")]
    pub backend: PressBackend,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PressBackend {
    Software,
    Hardware,
    Auto,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActPressResponse {
    pub ok: bool,
    pub keys_pressed: u32,
    pub elapsed_ms: u32,
    pub backend_used: String,
}

impl PressBackend {
    pub(in crate::m2::press) const fn to_backend(self) -> Backend {
        match self {
            Self::Software => Backend::Software,
            Self::Hardware => Backend::Hardware,
            Self::Auto => Backend::Auto,
        }
    }
}

pub(in crate::m2::press) const fn default_hold_ms() -> u32 {
    DEFAULT_HOLD_MS
}

pub(in crate::m2::press) const fn default_press_backend() -> PressBackend {
    PressBackend::Auto
}
