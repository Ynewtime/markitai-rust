use crate::{Asset, ConversionOutput, Document, Error, Result, config};
use chrono::{Local, SecondsFormat};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Component, Path};
use std::sync::Mutex;

static OUTPUT_LOCK: Mutex<()> = Mutex::new(());

pub fn check_path(path: &Path, allow_symlinks: bool) -> Result<()> {
    if allow_symlinks {
        return Ok(());
    }
    let absolute = std::path::absolute(path)?;
    for ancestor in absolute.ancestors() {
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor)
            && metadata.file_type().is_symlink()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if ancestor != absolute && metadata.uid() == 0 {
                    continue;
                }
            }
            return Err(Error::InvalidInput(format!(
                "Symlink access is disabled: {}",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

pub fn split_frontmatter(text: &str) -> (Map<String, Value>, &str) {
    if let Some(rest) = text.strip_prefix("---\n") {
        let mut offset = 0;
        for line in rest.split_inclusive('\n') {
            if line == "---\n" || line == "---" {
                if let Ok(Value::Object(meta)) = serde_yaml::from_str::<Value>(&rest[..offset]) {
                    return (meta, rest[offset + line.len()..].trim_start_matches('\n'));
                }
                break;
            }
            offset += line.len();
        }
    }
    (Map::new(), text)
}

pub fn render(frontmatter: &Map<String, Value>, markdown: &str) -> Result<String> {
    let mut yaml = String::new();
    for key in [
        "title",
        "source",
        "description",
        "tags",
        "markitai_processed",
        "fetch_strategy",
    ] {
        if let Some(value) = frontmatter.get(key) {
            yaml.push_str(
                &serde_yaml::to_string(&json!({key:value}))
                    .map_err(|e| Error::Conversion(e.to_string()))?,
            );
        }
    }
    for (key, value) in frontmatter {
        if ![
            "title",
            "source",
            "description",
            "tags",
            "markitai_processed",
            "fetch_strategy",
        ]
        .contains(&key.as_str())
        {
            yaml.push_str(
                &serde_yaml::to_string(&json!({key:value}))
                    .map_err(|e| Error::Conversion(e.to_string()))?,
            );
        }
    }
    Ok(format!("---\n{yaml}---\n\n{markdown}"))
}

/// Render the same document bytes for CLI stdout and persisted files.
pub fn content(result: &ConversionOutput, cfg: &Value, enhanced: bool) -> Result<String> {
    let (body, prefix) = if enhanced {
        (
            result.llm_markdown.as_deref().unwrap_or(&result.markdown),
            result.pure_llm_prefix.as_deref(),
        )
    } else {
        (result.markdown.as_str(), result.pure_prefix.as_deref())
    };
    if config::enabled(cfg, "/llm/pure") && cfg["output"]["profile"] != "okf" {
        Ok(format!("{}{body}", prefix.unwrap_or("")))
    } else {
        let metadata = if enhanced {
            &result.frontmatter
        } else {
            result
                .base_frontmatter
                .as_ref()
                .unwrap_or(&result.frontmatter)
        };
        render(metadata, body)
    }
}

pub(crate) fn apply_profiles(result: &mut ConversionOutput, cfg: &Value) {
    if let Some(markdown) = &mut result.llm_markdown {
        if let Some(base) = &mut result.base_frontmatter {
            crate::output_profiles::apply(&mut result.markdown, base, cfg);
        }
        crate::output_profiles::apply(markdown, &mut result.frontmatter, cfg);
        if result.frontmatter.is_empty() && config::enabled(cfg, "/llm/keep_base") {
            result.frontmatter = result.base_frontmatter.clone().unwrap_or_default();
        }
    } else {
        crate::output_profiles::apply(&mut result.markdown, &mut result.frontmatter, cfg);
        result.base_frontmatter = None;
    }
    if cfg["output"]["profile"] == "rag" {
        let mut notices = crate::output_profiles::table_warnings(&result.markdown);
        if let Some(markdown) = &result.llm_markdown {
            notices.extend(crate::output_profiles::table_warnings(markdown));
        }
        for warning in notices {
            if !result.warnings.contains(&warning) {
                result.warnings.push(warning);
            }
        }
    }
}

pub fn prepare(source: &str, name: &str, doc: &mut Document, cfg: &Value) -> ConversionOutput {
    let pure = config::enabled(cfg, "/llm/pure");
    let (mut meta, markdown, pure_prefix) = if pure {
        let (meta, body) = split_frontmatter(&doc.markdown);
        let prefix_len = doc.markdown.len() - body.len();
        (
            meta,
            body.to_owned(),
            (prefix_len > 0).then(|| doc.markdown[..prefix_len].to_owned()),
        )
    } else {
        (Map::new(), crate::markdown::normalize(&doc.markdown), None)
    };
    if !pure {
        let low_confidence = Path::new(name)
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| ["csv", "tsv", "xml"].contains(&v.to_ascii_lowercase().as_str()));
        // Basic workflow titles use the first heading. This deliberately keeps
        // its historical permissive syntax, including headings in code blocks.
        let heading = (!low_confidence)
            .then(|| {
                markdown.trim_start().lines().find_map(|line| {
                    if line.starts_with("# ") || line.starts_with("##") {
                        let title = line.trim_start_matches('#').trim().replace("**", "");
                        (!title.trim().is_empty()).then(|| title.trim().to_owned())
                    } else {
                        None
                    }
                })
            })
            .flatten();
        let title = doc
            .metadata
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .map(normalize_title)
            .or(heading)
            .unwrap_or_else(|| {
                if low_confidence {
                    name.to_owned()
                } else {
                    Path::new(name)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }
            });
        meta.insert(
            "title".into(),
            json!(title.split_whitespace().collect::<Vec<_>>().join(" ")),
        );
        let source_value = if crate::is_url(source) {
            redact_url(source)
        } else {
            name.to_string()
        };
        meta.insert("source".into(), json!(source_value));
        meta.insert(
            "markitai_processed".into(),
            json!(Local::now().to_rfc3339_opts(SecondsFormat::Millis, false)),
        );
        if crate::is_url(source) {
            for (key, value) in &doc.metadata {
                if ![
                    "title",
                    "source",
                    "description",
                    "tags",
                    "markitai_processed",
                    "language",
                    "format",
                ]
                .contains(&key.as_str())
                    && !value.is_null()
                {
                    meta.insert(key.clone(), value.clone());
                }
            }
        }
    }
    ConversionOutput {
        source: source.into(),
        markdown,
        pure_prefix,
        frontmatter: meta,
        warnings: std::mem::take(&mut doc.warnings),
        ..Default::default()
    }
}

fn normalize_title(title: &str) -> String {
    use std::sync::LazyLock;
    static LINKS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"!?\[([^\]]*)\]\([^)]+\)").unwrap());
    static TAGS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)</?(?:strong|em|b|i|code)>").unwrap());
    static HEADING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^#{1,6}\s+").unwrap());
    let text = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = HEADING.replace(&text, "");
    let text = LINKS.replace_all(&text, "$1");
    let text = TAGS.replace_all(&text, "");
    let mut text = text.trim();
    loop {
        let stripped = ["**", "__", "~~", "`", "*", "_"]
            .into_iter()
            .find_map(|wrapper| {
                text.strip_prefix(wrapper)?
                    .strip_suffix(wrapper)
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
            });
        match stripped {
            Some(value) => text = value,
            None => break,
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn redact_url(source: &str) -> String {
    let Ok(mut url) = url::Url::parse(source) else {
        return source.to_owned();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let pairs: Vec<_> = url
        .query_pairs()
        .map(|(key, value)| {
            let sensitive = [
                "token",
                "key",
                "secret",
                "password",
                "signature",
                "credential",
            ]
            .iter()
            .any(|part| key.to_lowercase().contains(part));
            (
                key.into_owned(),
                if sensitive {
                    "REDACTED".into()
                } else {
                    value.into_owned()
                },
            )
        })
        .collect();
    if !pairs.is_empty() {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    url.to_string()
}

pub fn url_name(source: &str, _meta: &Map<String, Value>) -> String {
    let parsed = url::Url::parse(source).ok();
    let host = parsed
        .as_ref()
        .and_then(|u| u.host_str())
        .unwrap_or("page")
        .replace('.', "_");
    let host = match parsed.as_ref().and_then(|u| u.port()) {
        Some(port) => format!("{host}_{port}"),
        None => host,
    };
    let segment = parsed
        .as_ref()
        .and_then(|u| u.path_segments()?.rfind(|s| !s.is_empty()));
    let raw = match segment {
        Some(path) if parsed.as_ref().is_some_and(|u| u.query().is_some()) => {
            format!("{host}_{path}")
        }
        Some(path) => path.to_owned(),
        None => host,
    };
    let safe: String = raw
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let safe: String = safe.trim_matches([' ', '.']).chars().take(200).collect();
    if safe.is_empty() {
        "unnamed".into()
    } else {
        safe
    }
}

pub fn should_skip(dir: &Path, name: &str, cfg: &Value) -> Result<bool> {
    if cfg.pointer("/output/on_conflict").and_then(Value::as_str) != Some("skip") {
        return Ok(false);
    }
    let name = cfg
        .pointer("/output/filename")
        .and_then(Value::as_str)
        .map(|s| s.strip_suffix(".md").unwrap_or(s))
        .or_else(|| cfg.pointer("/output/reserved_stem").and_then(Value::as_str))
        .unwrap_or(name);
    if Path::new(name).components().count() != 1 {
        return Err(Error::InvalidInput("Output name must be a filename".into()));
    }
    let base = dir.join(format!("{name}.md"));
    let llm = dir.join(format!("{name}.llm.md"));
    check_path(&base, config::enabled(cfg, "/output/allow_symlinks"))?;
    check_path(&llm, config::enabled(cfg, "/output/allow_symlinks"))?;
    Ok(base.exists() || llm.exists())
}

fn atomic_write(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("Output has no parent".into()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    if overwrite {
        temp.persist(path).map_err(|e| Error::Io(e.error))?;
    } else {
        temp.persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
    }
    Ok(())
}

pub fn write(
    dir: &Path,
    name: &str,
    result: &mut ConversionOutput,
    assets: &[Asset],
    cfg: &Value,
) -> Result<()> {
    let _guard = OUTPUT_LOCK
        .lock()
        .map_err(|_| Error::Conversion("Output lock poisoned".into()))?;
    let allow_symlinks = config::enabled(cfg, "/output/allow_symlinks");
    check_path(dir, allow_symlinks)?;
    let explicit_name = cfg.pointer("/output/filename").and_then(Value::as_str);
    let name = explicit_name
        .map(|name| name.strip_suffix(".md").unwrap_or(name))
        .or_else(|| cfg.pointer("/output/reserved_stem").and_then(Value::as_str))
        .unwrap_or(name);
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(Error::InvalidInput("Output name must be a filename".into()));
    }
    std::fs::create_dir_all(dir)?;
    let mode = cfg
        .pointer("/output/on_conflict")
        .and_then(Value::as_str)
        .unwrap_or("rename");
    let mut stem = name.to_owned();
    let mut revision = 2;
    loop {
        let base = dir.join(format!("{stem}.md"));
        let enhanced = dir.join(format!("{stem}.llm.md"));
        check_path(&base, allow_symlinks)?;
        check_path(&enhanced, allow_symlinks)?;
        if !base.exists() && !enhanced.exists() || mode == "overwrite" {
            break;
        }
        if mode == "skip" {
            result.skip_reason = Some("exists".into());
            result.markdown.clear();
            return Ok(());
        }
        stem = format!("{name}.v{revision}");
        revision += 1;
    }
    let visible = matches!(
        cfg.pointer("/output/profile").and_then(Value::as_str),
        Some("rag" | "obsidian")
    );
    let asset_prefix = if visible {
        "assets"
    } else {
        ".markitai/assets"
    };
    for asset in assets {
        let extension = Path::new(&asset.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("bin");
        let safe_extension: String = extension
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect();
        let digest = format!("{:x}", Sha256::digest(&asset.bytes));
        let filename = format!("{}.{}", &digest[..24], safe_extension);
        let path = dir.join(asset_prefix).join(&filename);
        check_path(&path, allow_symlinks)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            atomic_write(&path, &asset.bytes, false)?;
        }
        let before = format!("{asset_prefix}/{}", asset.name);
        let after = format!("{asset_prefix}/{filename}");
        let replace =
            |text: &str| crate::output_profiles::rewrite_asset_target(text, &before, &after);
        result.markdown = replace(&result.markdown);
        if let Some(md) = &mut result.llm_markdown {
            *md = replace(md);
        }
        if !result.assets.contains(&path) {
            result.assets.push(path);
        }
    }
    if result.llm_markdown.is_none() || config::enabled(cfg, "/llm/keep_base") {
        let path = dir.join(format!("{stem}.md"));
        let content = content(result, cfg, false)?;
        atomic_write(&path, content.as_bytes(), mode == "overwrite")?;
        result.output_path = Some(path);
    }
    if result.llm_markdown.is_some() {
        let path = if explicit_name.is_some() && !config::enabled(cfg, "/llm/keep_base") {
            dir.join(format!("{stem}.md"))
        } else {
            dir.join(format!("{stem}.llm.md"))
        };
        let content = content(result, cfg, true)?;
        atomic_write(&path, content.as_bytes(), mode == "overwrite")?;
        result.llm_output_path = Some(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_output_uses_workflow_title_and_preserves_original_frontmatter_as_body() {
        let input = "---\ntitle: Existing\ncustom: kept\n---\n\n# Heading\nBody  ";
        let mut document = Document {
            markdown: input.into(),
            ..Default::default()
        };
        document
            .metadata
            .insert("author".into(), json!("not a local frontmatter field"));
        let mut cfg = config::defaults();
        let normal = prepare("existing.md", "existing.md", &mut document, &cfg);
        assert_eq!(normal.frontmatter["title"], "Heading");
        assert!(!normal.frontmatter.contains_key("author"));
        assert_eq!(
            normal.markdown,
            "---\ntitle: Existing\ncustom: kept\n---\n\n# Heading\n\nBody\n"
        );
        cfg["llm"]["pure"] = json!(true);
        let mut pure = prepare("existing.md", "existing.md", &mut document, &cfg);
        assert_eq!(
            pure.frontmatter,
            json!({"title":"Existing","custom":"kept"})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(pure.markdown, "# Heading\nBody  ");
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "existing.md", &mut pure, &[], &cfg).unwrap();
        assert_eq!(
            std::fs::read_to_string(pure.output_path.unwrap()).unwrap(),
            input
        );
    }

    #[test]
    fn structured_data_uses_full_name_and_explicit_titles_remain_authoritative() {
        let cfg = config::defaults();
        for name in ["data.csv", "data.tsv", "data.XML"] {
            let mut doc = Document {
                markdown: "# Untrusted data\nbody".into(),
                ..Default::default()
            };
            assert_eq!(
                prepare(name, name, &mut doc, &cfg).frontmatter["title"],
                name
            );
            doc.metadata.insert(
                "title".into(),
                json!("**[Explicit](https://example.test)**"),
            );
            assert_eq!(
                prepare(name, name, &mut doc, &cfg).frontmatter["title"],
                "Explicit"
            );
        }
    }

    #[test]
    fn okf_uses_utc_and_adds_generated_identity_in_pure_mode() {
        let mut cfg = config::defaults();
        cfg["output"]["profile"] = json!("okf");
        let mut doc = Document {
            markdown: "# Heading".into(),
            ..Default::default()
        };
        let mut normal = prepare("file.txt", "file.txt", &mut doc, &cfg);
        apply_profiles(&mut normal, &cfg);
        assert!(
            normal.frontmatter["generated"]["at"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        assert!(!normal.frontmatter.contains_key("source"));
        cfg["llm"]["pure"] = json!(true);
        let mut pure = prepare("file.txt", "file.txt", &mut doc, &cfg);
        apply_profiles(&mut pure, &cfg);
        assert!(pure.frontmatter["generated"].get("at").is_none());
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "file.txt", &mut pure, &[], &cfg).unwrap();
        let written = std::fs::read_to_string(pure.output_path.unwrap()).unwrap();
        assert_eq!(split_frontmatter(&written).0["type"], "Document");
    }
    #[test]
    fn paired_conflicts_and_skip_preserve_existing_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf.llm.md"), "keep").unwrap();
        let mut result = ConversionOutput {
            markdown: "body".into(),
            ..Default::default()
        };
        let mut cfg = config::defaults();
        write(dir.path(), "report.pdf", &mut result, &[], &cfg).unwrap();
        assert!(result.output_path.unwrap().ends_with("report.pdf.v2.md"));
        cfg["output"]["on_conflict"] = json!("skip");
        let mut result = ConversionOutput::default();
        write(dir.path(), "report.pdf", &mut result, &[], &cfg).unwrap();
        assert_eq!(result.skip_reason.as_deref(), Some("exists"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("report.pdf.llm.md")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn malformed_frontmatter_remains_content() {
        let text = "---\ninvalid: [\n---\nbody";
        assert_eq!(split_frontmatter(text).1, text);
        assert_eq!(
            split_frontmatter("---\n---extra: kept\n---\nbody").0["---extra"],
            "kept"
        );
        assert_eq!(
            split_frontmatter("---\ntitle: 中文\n---\n\n# Body").0["title"],
            "中文"
        );
    }
    #[test]
    fn output_paths_cannot_escape_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            write(
                dir.path(),
                "../bad",
                &mut ConversionOutput::default(),
                &[],
                &config::defaults()
            )
            .is_err()
        );
    }
}
