use markitai_core::output::{Publication, write_with_publication};
use markitai_core::{
    ConversionOutput, ConvertContext, ConvertOptions, Error, convert_with_publication,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Skip {
    calls: AtomicUsize,
}
impl Publication for Skip {
    fn skip_existing(&self) -> bool {
        true
    }
    fn publish(&self, _path: &Path, _bytes: &[u8]) -> markitai_core::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Conversion(
            "a skipped publication must not be attempted".into(),
        ))
    }
}

#[test]
fn callback_skip_avoids_document_parsing_model_work_and_output_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("broken.ipynb");
    std::fs::write(&source, "{invalid notebook JSON").unwrap();
    let destination = root.path().join("output");
    let publication = Skip {
        calls: AtomicUsize::new(0),
    };
    // The ordinary conflict policy would convert this nonexistent destination.
    // Parsing this source or reaching the unconfigured model would return an error.
    let result = convert_with_publication(
        source.to_str().unwrap(),
        ConvertOptions {
            output_dir: Some(destination.clone()),
            config: Some(json!({
                "output":{"on_conflict":"overwrite"},
                "llm":{"enabled":true}, "cache":{"enabled":false},
                "prompts":{"dir":root.path().join("private-prompts")}
            })),
            ..Default::default()
        },
        ConvertContext::default(),
        Some(&publication),
    )
    .unwrap();
    assert_eq!(result.skip_reason.as_deref(), Some("exists"));
    assert_eq!(result.source, source.to_str().unwrap());
    assert!(result.markdown.is_empty());
    assert!(result.llm_markdown.is_none());
    assert!(result.output_path.is_none() && result.llm_output_path.is_none());
    assert!(result.assets.is_empty() && result.screenshots.is_empty());
    assert_eq!(result.usage.requests, 0);
    assert_eq!(publication.calls.load(Ordering::SeqCst), 0);
    assert!(!destination.exists());
    assert!(!root.path().join(".markitai").exists());
}

struct FailAt {
    fail_at: usize,
    calls: Mutex<Vec<(PathBuf, Vec<u8>)>>,
}
impl Publication for FailAt {
    fn skip_existing(&self) -> bool {
        false
    }
    fn publish(&self, path: &Path, bytes: &[u8]) -> markitai_core::Result<()> {
        let count = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((path.to_owned(), bytes.to_vec()));
            calls.len()
        };
        if count == self.fail_at {
            return Err(Error::Conversion("fixture publication denied".into()));
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

#[test]
fn paired_publication_failure_preserves_successful_member_and_never_falls_back_or_renames() {
    for fail_at in [1, 2] {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("fixed.md");
        let enhanced = root.path().join("fixed.llm.md");
        std::fs::write(&base, "old base bytes").unwrap();
        std::fs::write(&enhanced, "old enhanced bytes").unwrap();
        let base_bytes = "Base 世界 with two spaces  \n\n";
        let enhanced_bytes = "Enhanced body\n```text\n  literal  \n```\n";
        let mut result = ConversionOutput::default();
        result.markdown = base_bytes.into();
        result.llm_markdown = Some(enhanced_bytes.into());
        let publication = FailAt {
            fail_at,
            calls: Mutex::new(Vec::new()),
        };
        let cfg = json!({
            "llm":{"enabled":false,"pure":true,"keep_base":true},
            "output":{"on_conflict":"rename","reserved_stem":"fixed"}
        });
        let error = write_with_publication(
            root.path(),
            "source.txt",
            &mut result,
            &[],
            &cfg,
            Some(&publication),
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::Conversion(ref message) if message == "fixture publication denied")
        );
        let calls = publication.calls.lock().unwrap();
        assert_eq!(calls.len(), fail_at);
        assert_eq!(calls[0], (base.clone(), base_bytes.as_bytes().to_vec()));
        if fail_at == 1 {
            assert_eq!(std::fs::read(&base).unwrap(), b"old base bytes");
            assert!(result.output_path.is_none());
        } else {
            assert_eq!(
                calls[1],
                (enhanced.clone(), enhanced_bytes.as_bytes().to_vec())
            );
            assert_eq!(std::fs::read(&base).unwrap(), base_bytes.as_bytes());
            assert_eq!(result.output_path.as_ref(), Some(&base));
        }
        assert_eq!(std::fs::read(&enhanced).unwrap(), b"old enhanced bytes");
        assert!(result.llm_output_path.is_none());
        let mut names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                std::ffi::OsString::from("fixed.llm.md"),
                std::ffi::OsString::from("fixed.md")
            ]
        );
    }
}
