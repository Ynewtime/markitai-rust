mod asset_store;
pub mod config;
mod fetch;
pub mod fetch_cache;
pub mod formats;
mod images;
mod llm;
pub mod llm_cache;
mod llm_runtime;
mod markdown;
pub mod output;
mod output_profiles;
mod types;

pub use images::is_image_extension;
pub use llm_runtime::LlmRuntime;
pub use types::*;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

/// Report local model configuration without contacting a provider or exposing keys.
pub fn llm_capabilities(config: &Value) -> LlmCapabilities {
    llm::capabilities(config, &config::environment())
}

pub fn convert(source: &str, options: ConvertOptions) -> Result<ConversionOutput> {
    convert_with_context(source, options, ConvertContext::default())
}

/// Shares native run resources without adding fields to binding requests.
#[doc(hidden)]
pub fn convert_with_context(
    source: &str,
    options: ConvertOptions,
    context: ConvertContext<'_>,
) -> Result<ConversionOutput> {
    convert_with_publication(source, options, context, None)
}

/// Native callers may supply an already acquired document publication claim.
/// Existing conversion and serialized adapter entrypoints remain independent.
#[doc(hidden)]
pub fn convert_with_publication(
    source: &str,
    options: ConvertOptions,
    context: ConvertContext<'_>,
    publication: Option<&dyn output::Publication>,
) -> Result<ConversionOutput> {
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
    if config::enabled(&cfg, "/llm/enabled")
        && cfg
            .pointer("/llm/max_cost_per_document_usd")
            .and_then(Value::as_f64)
            .is_some_and(|v| v > 0.0)
    {
        return Err(Error::Unsupported("LLM cost limits require pricing support, which is not implemented in this development build".into()));
    }
    let is_url = is_url(source);
    if source.contains("://") && !is_url {
        return Err(Error::InvalidInput(
            "Only HTTP and HTTPS URLs are supported".into(),
        ));
    }
    let input_path = config::expand_home(Path::new(source));
    let image_input = !is_url
        && input_path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(is_image_extension);
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
        if meta.len() > 500 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "Input exceeds the 500 MiB limit".into(),
            ));
        }
    }
    if image_input
        && config::enabled(&cfg, "/ocr/enabled")
        && !config::enabled(&cfg, "/llm/enabled")
    {
        return Err(Error::Unsupported(
            "Local OCR is not implemented in this development build".into(),
        ));
    }
    if image_input && !config::enabled(&cfg, "/llm/enabled") {
        return Err(Error::ImageOnly(format!(
            "{} is an image file with no text to extract. Enable LLM (llm=True) or OCR (ocr=True) for content extraction.",
            input_path.file_name().unwrap_or_default().to_string_lossy()
        )));
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
        && match publication {
            Some(publication) => publication.skip_existing(),
            None => output::should_skip(dir, &name, &cfg)?,
        }
    {
        return Ok(ConversionOutput {
            source: source.into(),
            skip_reason: Some("exists".into()),
            duration: start.elapsed().as_secs_f64(),
            ..Default::default()
        });
    }
    if is_url && config::enabled(&cfg, "/screenshot/enabled") {
        return Err(Error::Unsupported(
            "Browser screenshots are not implemented in this development build".into(),
        ));
    }
    let mut vision = None;
    let mut fetch_cache_hit = false;
    let mut doc = if image_input {
        if config::enabled(&cfg, "/ocr/enabled")
            && config::environment()
                .get("MARKITAI_NO_VLM_OCR")
                .is_some_and(|value| {
                    ["1", "true", "yes", "on"].contains(&value.to_ascii_lowercase().as_str())
                })
        {
            return Err(Error::Unsupported("VLM OCR is disabled by MARKITAI_NO_VLM_OCR; the local OCR backend is not implemented yet".into()));
        }
        let (doc, image) = images::extract(&input_path, &cfg)?;
        vision = Some(image);
        doc
    } else if is_url {
        let fetched = fetch::fetch_with_context(source, &cfg, context.explicit_fetch_strategy)?;
        fetch_cache_hit = fetched.cache_hit;
        fetched.document
    } else {
        formats::extract(&input_path)?
    };
    let format = doc
        .metadata
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("");
    if format == "PDF" {
        if cfg["security"]["pdf_sanitize"] == "remove" {
            return Err(Error::Unsupported(
                "PDF hidden-text removal is not implemented in this development build".into(),
            ));
        }
        if config::enabled(&cfg, "/ocr/enabled") {
            return Err(Error::Unsupported(
                "PDF OCR is not implemented in this development build".into(),
            ));
        }
    }
    if matches!(format, "PDF" | "PPTX" | "PPT" | "PPTM" | "PPSX" | "PPSM")
        && config::enabled(&cfg, "/screenshot/enabled")
    {
        return Err(Error::Unsupported(
            "Document screenshots are not implemented in this development build".into(),
        ));
    }
    if !image_input {
        images::prepare_assets(&mut doc, &cfg);
    }
    if config::enabled(&cfg, "/llm/enabled")
        && (image_input || output_profiles::has_image_references(&doc.markdown))
    {
        for (path, feature) in [
            ("/image/alt_enabled", "Image alt text"),
            ("/image/desc_enabled", "Image descriptions"),
        ] {
            if config::enabled(&cfg, path) {
                return Err(Error::Unsupported(format!(
                    "{feature} is not implemented in this development build"
                )));
            }
        }
    }
    let fetch_strategy = if is_url {
        doc.metadata
            .get("fetch_strategy")
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        None
    };
    let mut result = output::prepare(source, &name, &mut doc, &cfg);
    result.fetch_cache_hit = fetch_cache_hit;
    result.fetch_strategy = fetch_strategy;
    if config::enabled(&cfg, "/llm/enabled") {
        let source_context = if is_url {
            output::redact_url(source)
        } else {
            name.clone()
        };
        result.base_frontmatter = Some(result.frontmatter.clone());
        let pure = config::enabled(&cfg, "/llm/pure");
        let input = if pure {
            &doc.markdown
        } else {
            &result.markdown
        };
        let without_cache = |(markdown, usage)| llm::Enhancement {
            markdown,
            usage,
            cache_hit: false,
            warnings: Vec::new(),
        };
        let enhanced = if let Some(image) = &vision {
            llm::enhance_image_with_source_and_runtime(
                input,
                &source_context,
                image.mime,
                &image.bytes,
                &cfg,
                context.llm_runtime,
            )
            .map(without_cache)
        } else if !pure && !is_url {
            llm::enhance_with_cache_and_runtime(
                input,
                &source_context,
                source,
                &cfg,
                context.llm_runtime,
            )
        } else {
            llm::enhance_with_source_and_runtime(input, &source_context, &cfg, context.llm_runtime)
                .map(without_cache)
        };
        match enhanced {
            Ok(enhancement) => {
                let markdown = enhancement.markdown;
                result.llm_cache_hit = enhancement.cache_hit;
                result.warnings.extend(enhancement.warnings);
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
                result.usage = enhancement.usage;
                if !result.llm_cache_hit {
                    result.warnings.push("LLM token usage is recorded; provider cost pricing is not yet available in this build.".into());
                }
            }
            Err(error) => {
                if matches!(error, Error::NoModelConfigured | Error::Unsupported(_)) {
                    return Err(error);
                }
                if cfg["llm"]["on_failure"] == "fail" {
                    output::apply_profiles(&mut result, &cfg);
                    if let Some(dir) = &output_dir {
                        output::write_with_publication(
                            dir,
                            &name,
                            &mut result,
                            &doc.assets,
                            &cfg,
                            publication,
                        )?;
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
        output::write_with_publication(&dir, &name, &mut result, &doc.assets, &cfg, publication)?;
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
