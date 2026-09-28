use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("MARKITAI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".markitai")
        })
}

pub fn expand_home(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(user) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        PathBuf::from(user).join(rest)
    } else {
        path.to_owned()
    }
}

pub fn defaults() -> Value {
    json!({
        "output":{"dir":null,"on_conflict":"rename","allow_symlinks":false,"report":null,"profile":null,"wikilinks":false},
        "llm":{"enabled":false,"pure":false,"keep_base":false,"on_failure":"fallback","model_list":[],"providers":[],"concurrency":10,"max_requests_per_document":50,"max_cost_per_document_usd":0.0,"max_vision_pages_per_document":0,"router_settings":{"timeout":120,"num_retries":2}},
        "image":{"alt_enabled":false,"desc_enabled":false,"compress":true,"quality":75,"stdout_persist":true},
        "ocr":{"enabled":false},"screenshot":{"enabled":false,"screenshot_only":false},
        "batch":{"concurrency":10,"url_concurrency":5,"max_depth":null,"glob":null},
        "cache":{"enabled":true},"fetch":{"strategy":"auto","remote_consent":"always","policy":{},"domain_profiles":{},"playwright":{},"jina":{},"defuddle":{},"cloudflare":{}},
        "security":{"pdf_sanitize":"warn"},"history":{"record":false},"office":{},"log":{},"prompts":{},"presets":{}
    })
}

pub fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, value) => *base = value,
    }
}

pub fn load(explicit: Option<&Path>, overrides: Option<Value>) -> Result<Value> {
    let env_path = std::env::var_os("MARKITAI_CONFIG").map(PathBuf::from);
    let local = PathBuf::from("markitai.json");
    let global = home().join("config.json");
    let selected = explicit
        .map(PathBuf::from)
        .or(env_path)
        .or_else(|| local.is_file().then_some(local))
        .or_else(|| global.is_file().then_some(global));
    let mut value = defaults();
    if let Some(path) = selected {
        let path = expand_home(&path);
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Config(format!("Cannot read config {}: {e}", path.display())))?;
        let data: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Config(format!("Invalid JSON in {}: {e}", path.display())))?;
        if !data.is_object() {
            return Err(Error::Config("Configuration must be a JSON object".into()));
        }
        merge(&mut value, data);
    }
    if let Some(overrides) = overrides {
        if !overrides.is_object() {
            return Err(Error::Config(
                "Configuration overrides must be a JSON object".into(),
            ));
        }
        merge(&mut value, overrides);
    }
    validate(&value)?;
    Ok(value)
}

pub fn validate(value: &Value) -> Result<()> {
    for section in [
        "output",
        "llm",
        "image",
        "ocr",
        "screenshot",
        "batch",
        "cache",
        "fetch",
        "security",
        "history",
        "office",
        "log",
        "prompts",
        "presets",
    ] {
        if !value.get(section).is_some_and(Value::is_object) {
            return Err(Error::Config(format!("{section} must be an object")));
        }
    }
    for (path, choices) in [
        ("/output/on_conflict", &["rename", "overwrite", "skip"][..]),
        ("/llm/on_failure", &["fallback", "fail"][..]),
        (
            "/fetch/strategy",
            &[
                "auto",
                "static",
                "playwright",
                "defuddle",
                "jina",
                "cloudflare",
            ][..],
        ),
        ("/fetch/remote_consent", &["always", "never", "ask"][..]),
        ("/security/pdf_sanitize", &["off", "warn", "remove"][..]),
    ] {
        if !value
            .pointer(path)
            .and_then(Value::as_str)
            .is_some_and(|s| choices.contains(&s))
        {
            return Err(Error::Config(format!(
                "Invalid {path}; expected {}",
                choices.join(", ")
            )));
        }
    }
    if let Some(profile) = value.pointer("/output/profile").filter(|v| !v.is_null())
        && !profile
            .as_str()
            .is_some_and(|s| ["rag", "obsidian", "okf"].contains(&s))
    {
        return Err(Error::Config(
            "output.profile must be rag, obsidian, okf or null".into(),
        ));
    }
    for path in [
        "/llm/enabled",
        "/llm/pure",
        "/llm/keep_base",
        "/image/alt_enabled",
        "/image/desc_enabled",
        "/ocr/enabled",
        "/screenshot/enabled",
        "/output/allow_symlinks",
    ] {
        if !value.pointer(path).is_some_and(Value::is_boolean) {
            return Err(Error::Config(format!("{path} must be a boolean")));
        }
    }
    for path in [
        "/batch/concurrency",
        "/batch/url_concurrency",
        "/llm/concurrency",
    ] {
        if !value
            .pointer(path)
            .and_then(Value::as_u64)
            .is_some_and(|n| n > 0 && n <= 1024)
        {
            return Err(Error::Config(format!(
                "{path} must be an integer between 1 and 1024"
            )));
        }
    }
    Ok(())
}

/// Read dotenv as data without changing the host process environment.
pub fn environment() -> HashMap<String, String> {
    let mut vars: HashMap<String, String> = std::env::vars().collect();
    for path in [PathBuf::from(".env"), home().join(".env")] {
        if let Ok(iter) = dotenvy::from_path_iter(path) {
            for (key, value) in iter.flatten() {
                vars.entry(key).or_insert(value);
            }
        }
    }
    vars
}

pub fn enabled(config: &Value, path: &str) -> bool {
    config
        .pointer(path)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_preserves_neighbors_and_replaces_arrays() {
        let mut cfg = defaults();
        merge(
            &mut cfg,
            json!({"llm":{"enabled":true,"model_list":[{"name":"test"}]}}),
        );
        assert_eq!(cfg["llm"]["on_failure"], "fallback");
        assert_eq!(cfg["llm"]["model_list"].as_array().unwrap().len(), 1);
        validate(&cfg).unwrap();
        cfg["output"]["on_conflict"] = json!("clobber");
        assert!(validate(&cfg).is_err());
    }
    #[test]
    fn invalid_config_never_defaults_silently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "{bad").unwrap();
        assert!(load(Some(&path), None).is_err());
    }
}
