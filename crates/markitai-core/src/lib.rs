pub mod config;
mod fetch;
pub mod formats;
mod llm;
mod markdown;
pub mod output;
mod output_profiles;
mod types;

pub use types::*;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

pub fn convert(source: &str, options: ConvertOptions) -> Result<ConversionOutput> {
    let start = Instant::now();
    if source.trim().is_empty() {
        return Err(Error::InvalidInput("Input cannot be empty".into()));
    }
    let mut cfg = if let Some(value) = options.config {
        if !value.is_object() {
            return Err(Error::Config("config must be an object".into()));
        }
        config::normalize(&value)?
    } else {
        config::load(None, None)?
    };
    for (path, value) in [
        ("/llm/enabled", options.llm),
        ("/ocr/enabled", options.ocr),
        ("/screenshot/enabled", options.screenshot),
        ("/image/alt_enabled", options.alt),
        ("/image/desc_enabled", options.desc),
    ] {
        if let Some(value) = value {
            *cfg.pointer_mut(path)
                .ok_or_else(|| Error::Config(format!("Invalid config at {path}")))? =
                Value::Bool(value);
        }
    }
    if let Some(profile) = options.profile {
        if !matches!(profile.as_str(), "rag" | "obsidian" | "okf") {
            return Err(Error::Config(format!("Invalid output profile: {profile}")));
        }
        cfg["output"]["profile"] = json!(profile);
    }
    if cfg
        .pointer("/security/pdf_sanitize")
        .and_then(Value::as_str)
        == Some("remove")
    {
        return Err(Error::Unsupported(
            "PDF hidden-text removal is not implemented in this development build".into(),
        ));
    }
    if config::enabled(&cfg, "/llm/enabled")
        && cfg
            .pointer("/llm/max_cost_per_document_usd")
            .and_then(Value::as_f64)
            .is_some_and(|v| v > 0.0)
    {
        return Err(Error::Unsupported("LLM cost limits require pricing support, which is not implemented in this development build".into()));
    }
    if config::enabled(&cfg, "/llm/enabled")
        && cfg
            .pointer("/llm/model_list")
            .and_then(Value::as_array)
            .is_some_and(|v| v.len() > 1)
    {
        return Err(Error::Unsupported(
            "Multiple-model routing is not implemented in this development build".into(),
        ));
    }
    for (path, feature) in [
        ("/ocr/enabled", "OCR"),
        ("/screenshot/enabled", "Screenshots"),
        ("/image/alt_enabled", "Image alt text"),
        ("/image/desc_enabled", "Image descriptions"),
    ] {
        if config::enabled(&cfg, path) {
            return Err(Error::Unsupported(format!(
                "{feature} is not implemented in this development build"
            )));
        }
    }
    let is_url = is_url(source);
    if source.contains("://") && !is_url {
        return Err(Error::InvalidInput(
            "Only HTTP and HTTPS URLs are supported".into(),
        ));
    }
    let input_path = config::expand_home(Path::new(source));
    let output_dir = options.output_dir.map(|path| config::expand_home(&path));
    if !is_url {
        let path = &input_path;
        output::check_path(path, config::enabled(&cfg, "/output/allow_symlinks"))?;
        let meta = std::fs::metadata(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(source.into())
            } else {
                Error::Io(error)
            }
        })?;
        if meta.is_dir() {
            return Err(Error::IsDirectory(source.into()));
        }
        if !meta.is_file() {
            return Err(Error::InvalidInput(format!(
                "Input is not a file: {source}"
            )));
        }
        if meta.len() > 100 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "Input exceeds the 100 MiB limit".into(),
            ));
        }
    }
    let name = if is_url {
        output::url_name(source, &Default::default())
    } else {
        input_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    if let Some(dir) = &output_dir
        && output::should_skip(dir, &name, &cfg)?
    {
        return Ok(ConversionOutput {
            source: source.into(),
            skip_reason: Some("exists".into()),
            duration: start.elapsed().as_secs_f64(),
            ..Default::default()
        });
    }
    let mut doc = if is_url {
        fetch::fetch(source, &cfg)?
    } else {
        formats::extract(&input_path)?
    };
    let mut result = output::prepare(source, &name, &mut doc, &cfg);
    if config::enabled(&cfg, "/llm/enabled") {
        result.base_frontmatter = Some(result.frontmatter.clone());
        let pure = config::enabled(&cfg, "/llm/pure");
        let input = if pure {
            &doc.markdown
        } else {
            &result.markdown
        };
        match llm::enhance(input, &cfg) {
            Ok((markdown, usage)) => {
                let (meta, body) = output::split_frontmatter(&markdown);
                if pure {
                    let prefix_len = markdown.len() - body.len();
                    result.pure_llm_prefix =
                        (prefix_len > 0).then(|| markdown[..prefix_len].to_owned());
                    result.frontmatter = meta;
                } else {
                    result
                        .frontmatter
                        .extend(meta.into_iter().filter(|(key, _)| {
                            !["source", "markitai_processed"].contains(&key.as_str())
                        }));
                }
                result.llm_markdown = Some(if pure {
                    body.to_owned()
                } else {
                    crate::markdown::normalize(body)
                });
                result.usage = usage;
                result.warnings.push("LLM token usage is recorded; provider cost pricing is not yet available in this build.".into());
            }
            Err(error) => {
                if matches!(error, Error::NoModelConfigured | Error::Unsupported(_)) {
                    return Err(error);
                }
                if cfg["llm"]["on_failure"] == "fail" {
                    output::apply_profiles(&mut result, &cfg);
                    if let Some(dir) = &output_dir {
                        output::write(dir, &name, &mut result, &doc.assets, &cfg)?;
                    }
                    return Err(error);
                }
                result.warnings.push(format!(
                    "LLM enhancement failed; base Markdown retained: {error}"
                ));
            }
        }
    }
    output::apply_profiles(&mut result, &cfg);
    if let Some(dir) = output_dir {
        output::write(&dir, &name, &mut result, &doc.assets, &cfg)?;
    }
    result.duration = start.elapsed().as_secs_f64();
    Ok(result)
}

pub fn is_url(source: &str) -> bool {
    source
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        || source
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
}

/// Shared native adapter protocol. Errors are values; no host stdout is used.
pub fn convert_json(request: &str) -> String {
    let result = serde_json::from_str::<Request>(request)
        .map_err(Error::from)
        .and_then(|r| convert(&r.source, r.options));
    match result {
        Ok(result) => json!({"ok":true,"result":result}).to_string(),
        Err(error) => json!({"ok":false,"error":{"code":error.code(),"message":error.to_string()}})
            .to_string(),
    }
}
