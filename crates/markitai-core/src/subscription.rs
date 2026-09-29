//! Optional subscription providers communicate with an installed official runtime.
mod copilot;
mod process;
#[cfg(test)]
mod tests;

pub use copilot::{complete, models, status};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const COPILOT_PROTOCOL: u64 = 3;
pub const COPILOT_SCHEMA_REVISION: &str = "f5d9685f55286061e12763ed15a73c4469609881";
pub const COPILOT_CLI_VERSION: &str = "1.0.90-2";

/// Credentials are deliberately excluded from Debug and serialization.
#[derive(Clone)]
pub struct CopilotConfig {
    executable: PathBuf,
    environment: HashMap<String, String>,
    token: Option<String>,
}
impl CopilotConfig {
    pub fn from_env(env: &HashMap<String, String>) -> crate::Result<Self> {
        let executable = if let Some(path) = env.get("COPILOT_CLI_PATH") {
            PathBuf::from(path)
        } else {
            let name = if cfg!(windows) {
                "copilot.exe"
            } else {
                "copilot"
            };
            env.get("PATH")
                .and_then(|paths| {
                    std::env::split_paths(paths)
                        .map(|dir| dir.join(name))
                        .find(|path| path.is_file())
                })
                .ok_or_else(|| {
                    crate::Error::Unsupported(
                        "Copilot CLI is unavailable; install the supported official runtime".into(),
                    )
                })?
        };
        let executable = executable
            .canonicalize()
            .map_err(|_| crate::Error::InvalidInput("Copilot executable is unavailable".into()))?;
        if !executable.is_file() {
            return Err(crate::Error::InvalidInput(
                "Copilot executable must be a regular file".into(),
            ));
        }
        let token = ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .find_map(|key| env.get(*key).filter(|value| !value.is_empty()).cloned());
        if token
            .as_ref()
            .is_some_and(|value| value.len() > 16 * 1024 || value.contains(['\r', '\n', '\0']))
        {
            return Err(crate::Error::InvalidInput(
                "Copilot credential is invalid or oversized".into(),
            ));
        }
        // Retain OS process prerequisites and the explicitly selected official auth
        // home. Other model-provider variables must not silently turn this into BYOK.
        let mut environment = HashMap::new();
        for key in [
            "HOME",
            "PATH",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "SystemRoot",
            "SYSTEMROOT",
            "WINDIR",
            "TMPDIR",
            "TEMP",
            "TMP",
            "LANG",
            "LC_ALL",
            "COPILOT_HOME",
            "COPILOT_CACHE_HOME",
        ] {
            if let Some(value) = env.get(key) {
                environment.insert(key.into(), value.clone());
            } else if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        Ok(Self {
            executable,
            environment,
            token,
        })
    }
    pub fn executable(&self) -> &std::path::Path {
        &self.executable
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthStatus {
    pub provider: &'static str,
    pub authenticated: bool,
    pub user: Option<String>,
    pub expires_at: Option<String>,
    pub error: Option<String>,
    pub details: serde_json::Value,
}
#[derive(Clone, Debug, Serialize)]
pub struct Model {
    pub model: String,
    pub label: String,
    pub supports_vision: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct UsageEvidence {
    /// One entry for each distinct runtime-reported API call, not an estimate.
    pub calls: Vec<ObservedCall>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObservedCall {
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Authentication,
    Unsupported,
    Transport,
    Timeout,
    Cancelled,
    Protocol,
    Permission,
    ResourceLimit,
    Refusal,
    Truncated,
    RateLimit,
    InvalidRequest,
}
#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    pub error: crate::Error,
    pub usage: UsageEvidence,
}
impl Failure {
    fn new(kind: FailureKind, message: &'static str) -> Self {
        Self {
            kind,
            error: crate::Error::Conversion(message.into()),
            usage: UsageEvidence::default(),
        }
    }
}
#[derive(Debug)]
pub struct Completion {
    pub text: String,
    pub usage: UsageEvidence,
    pub warnings: Vec<String>,
}
pub struct Request<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    pub images: &'a [(&'a str, &'a [u8])],
    pub timeout: Duration,
    pub cancel: Option<&'a AtomicBool>,
}

pub mod claude;
