use markitai_core::{ConvertOptions, Error, convert, convert_json};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;

fn options() -> ConvertOptions {
    ConvertOptions {
        config: Some(json!({})),
        llm: Some(false),
        ..Default::default()
    }
}

#[test]
fn memory_and_disk_outputs_share_content_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("source.md");
    std::fs::write(
        &input,
        "# 标题\n\nA **paragraph** with a [link](https://example.com).\n",
    )
    .unwrap();
    let memory = convert(input.to_str().unwrap(), options()).unwrap();
    assert!(memory.output_path.is_none());
    assert!(memory.assets.is_empty());
    assert_eq!(memory.frontmatter["source"], "source.md");
    let output_dir = dir.path().join("out");
    let disk = convert(
        input.to_str().unwrap(),
        ConvertOptions {
            output_dir: Some(output_dir.clone()),
            ..options()
        },
    )
    .unwrap();
    assert_eq!(memory.markdown, disk.markdown);
    assert_eq!(disk.output_path, Some(output_dir.join("source.md.md")));
    let written = std::fs::read_to_string(disk.output_path.unwrap()).unwrap();
    assert!(written.starts_with("---\ntitle:"));
    assert!(written.ends_with(&disk.markdown));
}

#[test]
fn native_request_rejects_unknown_options_and_preserves_error_codes() {
    let missing: Value = serde_json::from_str(&convert_json(
        r#"{"source":"/missing-markitai-fixture.md","options":{"config":{}}}"#,
    ))
    .unwrap();
    assert_eq!(missing["error"]["code"], "not_found");
    let unknown: Value = serde_json::from_str(&convert_json(
        r#"{"source":"x","options":{"silently_ignored":true}}"#,
    ))
    .unwrap();
    assert_eq!(unknown["ok"], false);
}

fn llm_server(status: u16, payload: &'static str) -> (String, std::thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let request = loop {
            let mut chunk = [0; 4096];
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(offset) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..offset]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                if bytes.len() >= offset + 4 + length {
                    assert!(header.starts_with("POST /v1/chat/completions "));
                    break serde_json::from_slice::<Value>(&bytes[offset + 4..offset + 4 + length])
                        .unwrap();
                }
            }
        };
        write!(stream,"HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).unwrap();
        request
    });
    (format!("http://{address}/v1"), handle)
}

#[test]
fn llm_native_http_contract_usage_and_failure_policy() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.txt");
    std::fs::write(&input, "Body with facts.").unwrap();
    for (status, payload, policy) in [
        (
            200,
            r##"{"choices":[{"message":{"content":"# Enhanced\n\nBody with facts."}}],"usage":{"prompt_tokens":12,"completion_tokens":9}}"##,
            "fallback",
        ),
        (401, r#"{"error":{"message":"denied"}}"#, "fallback"),
        (500, r#"{"error":{"message":"unavailable"}}"#, "fail"),
    ] {
        let (base, server) = llm_server(status, payload);
        let cfg = json!({"llm":{"enabled":true,"on_failure":policy,"model_list":[{"model_name":"test","litellm_params":{"model":"openai/test-model","api_base":base,"api_key":"test-key"}}]}});
        let result = convert(
            input.to_str().unwrap(),
            ConvertOptions {
                config: Some(cfg),
                output_dir: Some(dir.path().join(format!("out-{status}"))),
                ..Default::default()
            },
        );
        let request = server.join().unwrap();
        assert_eq!(request["model"], "test-model");
        assert_eq!(request["messages"][1]["content"], "Body with facts.");
        if status == 200 {
            let result = result.unwrap();
            assert_eq!(result.usage.input_tokens, 12);
            assert_eq!(result.usage.output_tokens, 9);
            assert!(result.output_path.is_none());
            assert!(result.llm_output_path.unwrap().is_file());
        } else if policy == "fallback" {
            let result = result.unwrap();
            assert!(result.llm_markdown.is_none());
            assert!(!result.warnings.is_empty());
            assert!(result.output_path.unwrap().is_file());
        } else {
            assert!(matches!(result, Err(Error::Conversion(_))));
            assert!(
                dir.path()
                    .join(format!("out-{status}/input.txt.md"))
                    .is_file()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn output_parent_symlinks_do_not_modify_external_files() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.md");
    std::fs::write(&input, "content").unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(
        convert(
            input.to_str().unwrap(),
            ConvertOptions {
                output_dir: Some(link),
                ..options()
            }
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn invalid_override_structure_is_an_error_and_skip_avoids_parsing_or_llm() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("broken.ipynb");
    std::fs::write(&input, "not JSON").unwrap();
    let invalid = convert(
        input.to_str().unwrap(),
        ConvertOptions {
            config: Some(json!({"output":false})),
            profile: Some("rag".into()),
            ..Default::default()
        },
    );
    assert!(matches!(invalid, Err(Error::Config(_))));
    let output = dir.path().join("out");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("broken.ipynb.llm.md"), "must survive").unwrap();
    let skipped = convert(
        input.to_str().unwrap(),
        ConvertOptions {
            output_dir: Some(output.clone()),
            config: Some(json!({"output":{"on_conflict":"skip"},"llm":{"enabled":true}})),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(skipped.skip_reason.as_deref(), Some("exists"));
    assert!(skipped.markdown.is_empty());
    assert!(skipped.llm_markdown.is_none());
    assert_eq!(skipped.usage.requests, 0);
    assert_eq!(
        std::fs::read_to_string(output.join("broken.ipynb.llm.md")).unwrap(),
        "must survive"
    );
}

#[test]
fn url_names_and_explicit_pure_output_follow_existing_contracts() {
    for (url, name) in [
        ("https://example.com/page.html", "page.html"),
        ("https://example.com/path/to/doc", "doc"),
        ("https://example.com/", "example_com"),
        ("https://youtube.com/watch?v=abc", "youtube_com_watch"),
        (
            "https://example.com:8080/search?q=x",
            "example_com_8080_search",
        ),
    ] {
        assert_eq!(
            markitai_core::output::url_name(url, &Default::default()),
            name
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.txt");
    std::fs::write(&input, "Pure body").unwrap();
    let output = dir.path().join("out");
    let result = convert(
        input.to_str().unwrap(),
        ConvertOptions {
            output_dir: Some(output.clone()),
            config: Some(json!({"output":{"filename":"chosen.md"},"llm":{"pure":true}})),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.output_path, Some(output.join("chosen.md")));
    assert_eq!(
        std::fs::read_to_string(output.join("chosen.md")).unwrap(),
        "Pure body"
    );
}
