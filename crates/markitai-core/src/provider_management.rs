//! Bounded provider discovery and one-deployment connection checks.
//!
//! These calls use process credentials only. They do not load configuration,
//! prompt files, dotenv or document caches. Copilot delegates its auth store
//! to the installed official runtime; this module never parses that store.
mod cache;
mod discovery;
#[cfg(test)]
mod tests;

use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::HashMap;

const MAX_FIELD: usize = 8192;
const DETECTED: [(&str, &str, &str); 5] = [
    ("openai", "OPENAI_API_KEY", "openai/gpt-5.6-luna"),
    (
        "anthropic",
        "ANTHROPIC_API_KEY",
        "anthropic/claude-haiku-4-5",
    ),
    (
        "gemini",
        "GEMINI_API_KEY",
        "gemini/gemini-flash-lite-latest",
    ),
    ("deepseek", "DEEPSEEK_API_KEY", "deepseek/deepseek-v4-flash"),
    (
        "openrouter",
        "OPENROUTER_API_KEY",
        "openrouter/google/gemini-3.1-flash-lite",
    ),
];

/// Conventional labels, without credential or account information.
pub fn provider_label(provider: &str) -> String {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "gemini" => "Gemini",
        "deepseek" => "DeepSeek",
        "openrouter" => "OpenRouter",
        "azure" => "Azure",
        "ollama" => "Ollama",
        "custom" => "Custom",
        "claude-agent" => "Claude Agent",
        "copilot" => "Copilot",
        "chatgpt" => "ChatGPT",
        _ => "Unknown provider",
    }
    .into()
}

/// Discovery bases; Gemini and Ollama inference use different endpoint paths.
pub fn provider_default_base(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("https://api.openai.com/v1"),
        "anthropic" => Some("https://api.anthropic.com/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        "deepseek" => Some("https://api.deepseek.com"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "ollama" => Some("http://127.0.0.1:11434"),
        _ => None,
    }
}

/// Quick-add candidates from nonempty process environment keys, without I/O.
pub fn detected() -> Vec<Value> {
    DETECTED.iter().filter(|(_, key, _)| std::env::var(key).is_ok_and(|v| !v.trim().is_empty()))
        .map(|(provider, _, model)| json!({"provider":provider,"model":model,"label":provider_label(provider),"requires_api_key":false})).collect()
}

pub(super) fn field<'a>(request: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= MAX_FIELD && !value.contains('\0') => {
            Ok(Some(value))
        }
        _ => Err(Error::InvalidInput(
            "Provider request contains an invalid or oversized field".into(),
        )),
    }
}
fn validate(request: &Value, keys: &[&str]) -> Result<()> {
    let object = request
        .as_object()
        .ok_or_else(|| Error::InvalidInput("Provider request must be an object".into()))?;
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(Error::InvalidInput(
            "Provider request contains an unknown field".into(),
        ));
    }
    Ok(())
}
pub(super) fn resolve(
    value: Option<&str>,
    fallback: Option<&str>,
    env: &HashMap<String, String>,
) -> Result<Option<String>> {
    let resolved = crate::config::resolve_optional(value, fallback, env, true).map_err(|_| {
        Error::InvalidInput(
            "Provider credential or endpoint environment reference is unavailable".into(),
        )
    })?;
    if resolved
        .as_ref()
        .is_some_and(|value| value.len() > MAX_FIELD || value.chars().any(char::is_control))
    {
        return Err(Error::InvalidInput(
            "Provider credential or endpoint is invalid or oversized".into(),
        ));
    }
    Ok(resolved)
}
pub(super) fn checked_url(base: &str) -> Result<url::Url> {
    let value = url::Url::parse(base).map_err(|_| {
        Error::InvalidInput("Provider endpoint must be an absolute HTTP(S) URL".into())
    })?;
    if !matches!(value.scheme(), "http" | "https")
        || value.host_str().is_none()
        || !value.username().is_empty()
        || value.password().is_some()
        || value.fragment().is_some()
    {
        return Err(Error::InvalidInput(
            "Provider endpoint must use HTTP(S) without user information or a fragment".into(),
        ));
    }
    Ok(value)
}

/// Discover model identifiers using a credential-isolated in-memory cache.
/// The input contains provider, optional api_key/api_base and optional refresh.
pub fn discover(request: &Value) -> Result<Value> {
    validate(request, &["provider", "api_key", "api_base", "refresh"])?;
    let provider = field(request, "provider")?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::InvalidInput("Provider is required".into()))?
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        provider.as_str(),
        "openai"
            | "anthropic"
            | "gemini"
            | "deepseek"
            | "openrouter"
            | "azure"
            | "ollama"
            | "custom"
            | "claude-agent"
            | "copilot"
            | "chatgpt"
    ) {
        return Err(Error::InvalidInput("Unknown provider".into()));
    }
    let refresh = match request.get("refresh") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err(Error::InvalidInput("refresh must be a boolean".into())),
    };
    let env: HashMap<String, String> = std::env::vars().collect();
    if provider == "copilot" {
        if field(request, "api_key")?.is_some_and(|value| !value.is_empty())
            || field(request, "api_base")?.is_some_and(|value| !value.is_empty())
        {
            return Err(Error::InvalidInput("Copilot discovery uses official CLI authentication, not API credentials or a base URL".into()));
        }
        // A stored login can change within one HOME. No discovery cache keyed by
        // a filesystem path can establish account identity, so query each time.
        let result = crate::subscription::CopilotConfig::from_env(&env).and_then(|config| {
            crate::subscription::models(&config, std::time::Duration::from_secs(15))
                .map_err(|failure| failure.error)
        });
        return Ok(match result {
            Ok(models) => {
                json!({"provider":"copilot","status":"ok","source":"official_cli","authoritative":true,"cached":false,"stale":false,"models":models,"detail":"Models reported by the authenticated official Copilot runtime"})
            }
            Err(_) => {
                json!({"provider":"copilot","status":"unavailable","source":"official_cli","authoritative":false,"cached":false,"stale":false,"models":[],"detail":"Official Copilot runtime or authentication is unavailable"})
            }
        });
    }
    if provider == "claude-agent" {
        if field(request, "api_key")?.is_some_and(|value| !value.is_empty())
            || field(request, "api_base")?.is_some_and(|value| !value.is_empty())
        {
            return Err(Error::InvalidInput("Claude discovery uses official CLI authentication, not API credentials or a base URL".into()));
        }
        // A stored subscription can change accounts independently of this process.
        let result = crate::subscription::claude::Config::from_env(&env).and_then(|config| {
            crate::subscription::claude::models(&config, std::time::Duration::from_secs(15))
                .map_err(|failure| failure.error)
        });
        return Ok(match result {
            Ok(models) => {
                json!({"provider":"claude-agent","status":"ok","source":"official_cli","authoritative":true,"cached":false,"stale":false,"models":models,"detail":"Models reported by the authenticated official Claude runtime"})
            }
            Err(_) => {
                json!({"provider":"claude-agent","status":"unavailable","source":"official_cli","authoritative":false,"cached":false,"stale":false,"models":[],"detail":"Official Claude runtime or subscription authentication is unavailable"})
            }
        });
    }
    let variable = match provider.as_str() {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "gemini" => Some("GEMINI_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "azure" => Some("AZURE_API_KEY"),
        "ollama" => Some("OLLAMA_API_KEY"),
        _ => None,
    };
    let key = resolve(field(request, "api_key")?, variable, &env)?;
    let base = resolve(field(request, "api_base")?, None, &env)?
        .or_else(|| provider_default_base(&provider).map(str::to_owned));
    if let Some(base) = &base {
        checked_url(base)?;
    }
    if provider == "chatgpt" {
        return Ok(discovery::unavailable(
            &provider,
            "This provider requires an unavailable OAuth or local runtime integration",
        ));
    }
    let Some(base) = base.filter(|value| !value.trim().is_empty()) else {
        return Ok(discovery::unavailable(
            &provider,
            "An API endpoint is required",
        ));
    };
    cache::global().discover(&provider, &base, key.as_deref(), refresh, || {
        discovery::load(&provider, &base, key.as_deref())
    })
}

/// Make one short request to exactly the requested deployment, without retry,
/// fallback, prompt loading, document conversion or persistent cache access.
pub fn probe(request: &Value) -> Result<Value> {
    validate(request, &["model", "api_key", "api_base"])?;
    let model = field(request, "model")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::InvalidInput("Model is required".into()))?;
    field(request, "api_key")?;
    field(request, "api_base")?;
    if model.chars().any(char::is_control) || model.len() > 1024 {
        return Err(Error::InvalidInput(
            "Model contains invalid characters or exceeds 1024 bytes".into(),
        ));
    }
    match crate::llm::service_probe(request) {
        Ok(()) => Ok(
            json!({"ok":true,"detail":format!("{} responded", model.chars().take(280).collect::<String>())}),
        ),
        Err(error) => {
            // This helper's errors are deliberately fixed text or HTTP status,
            // never reqwest URLs, response bodies or user-supplied credentials.
            let detail: String = match error {
                Error::Unsupported(_) => {
                    "This model provider is not supported by the native runtime".into()
                }
                Error::Config(_) | Error::NoModelConfigured | Error::InvalidInput(_) => {
                    "Model credentials or endpoint configuration are invalid or unavailable".into()
                }
                Error::Conversion(message) => message.chars().take(300).collect(),
                _ => "Model connection test failed".into(),
            };
            Ok(json!({"ok":false,"detail":detail}))
        }
    }
}
