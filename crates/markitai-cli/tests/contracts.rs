use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn invoke(root: &Path, args: &[&str]) -> Output {
    invoke_env(root, args, &[])
}
fn invoke_env(root: &Path, args: &[&str], overrides: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_markitai"))
        .current_dir(root)
        .env("MARKITAI_HOME", root.join("home"))
        .env_remove("MARKITAI_CONFIG")
        .env_remove("MODEL")
        .env_remove("MARKITAI_PURE")
        .env_remove("MARKITAI_RECORD_HISTORY")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .envs(overrides.iter().copied())
        .args(args)
        .output()
        .expect("CLI starts")
}
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "# Contract\n\nHello 世界\n").unwrap();
    dir
}
#[test]
fn single_stdout_is_markdown_and_explicit_file_target_is_exact() {
    let dir = fixture();
    let output = invoke(dir.path(), &["note.txt", "--pure"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "# Contract\n\nHello 世界\n"
    );
    let output = invoke(dir.path(), &["note.txt", "-o", "chosen.md", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["version"], "1.0");
    assert_eq!(body["ok"], true);
    assert!(dir.path().join("chosen.md").is_file());
    assert_eq!(body["items"][0]["status"], "completed");
}
#[test]
fn single_runtime_error_retains_json_and_usage_error_has_none() {
    let dir = fixture();
    let output = invoke(dir.path(), &["missing.txt", "-o", "out", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["totals"]["failed"], 1);
    let output = invoke(dir.path(), &["note.txt", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
#[test]
fn batch_keeps_relative_directories_and_partial_failure_exit() {
    let dir = fixture();
    std::fs::create_dir_all(dir.path().join("input/nested")).unwrap();
    std::fs::write(dir.path().join("input/nested/good.txt"), "Good\n").unwrap();
    std::fs::write(dir.path().join("input/bad.ipynb"), "{bad").unwrap();
    let output = invoke(dir.path(), &["input", "-o", "out", "--json", "-j", "2"]);
    assert_eq!(
        output.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["totals"]["completed"], 1);
    assert_eq!(body["totals"]["failed"], 1);
    assert!(dir.path().join("out/nested/good.txt.md").is_file());
}
#[test]
fn config_overrides_are_transient_and_secret_values_hidden() {
    let dir = fixture();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"llm":{"model_list":[{"litellm_params":{"api_key":"do-not-print","model":"test"}}]}}"#,
    )
    .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "-c",
            "config.json",
            "--config-json",
            r#"{"llm":{"enabled":true}}"#,
            "config",
            "list",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("do-not-print"));
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["llm"]["enabled"], true);
    let persisted = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(!persisted.contains("enabled"));
}
#[test]
fn unsupported_options_fail_instead_of_ignoring_requests() {
    let dir = fixture();
    let output = invoke(dir.path(), &["note.txt", "--resume"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented"));
    assert!(output.stdout.is_empty());
}
#[test]
fn dry_run_does_not_create_outputs() {
    let dir = fixture();
    let output = invoke(dir.path(), &["note.txt", "-o", "preview", "--dry-run"]);
    assert!(output.status.success());
    assert!(!dir.path().join("preview").exists());
}

#[test]
fn url_batch_overwrite_preserves_both_custom_named_results() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let dir = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let started = std::time::Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            started.elapsed() < std::time::Duration::from_secs(15),
                            "CLI did not connect to fixture server"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("Fixture accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut buffer = [0u8; 8192];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]);
            let body = if request.contains("?id=first") {
                "First result\n"
            } else {
                "Second result\n"
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    std::fs::write(
        dir.path().join("sources.urls"),
        format!("http://{address}/page?id=first shared\nhttp://{address}/page?id=second shared\n"),
    )
    .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "sources.urls",
            "-o",
            "out",
            "--json",
            "--config-json",
            r#"{"output":{"on_conflict":"overwrite"}}"#,
        ],
    );
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["totals"]["completed"], 2);
    assert_ne!(body["items"][0]["output"], body["items"][1]["output"]);
    let first = std::fs::read_to_string(dir.path().join("out/shared.md")).unwrap();
    let second = std::fs::read_to_string(dir.path().join("out/shared.v2.md")).unwrap();
    assert!(first.contains("First result"));
    assert!(second.contains("Second result"));
}

#[test]
fn config_set_redacts_nested_credentials_when_echoing_whole_section() {
    let dir = fixture();
    let output = invoke(
        dir.path(),
        &[
            "-c",
            "new.json",
            "config",
            "set",
            "llm",
            r#"{"enabled":false,"model_list":[{"litellm_params":{"api_key":"nested-secret","model":"test"}}]}"#,
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("nested-secret"));
    assert!(
        std::fs::read_to_string(dir.path().join("new.json"))
            .unwrap()
            .contains("nested-secret")
    );
}

#[test]
fn directory_filters_on_url_lists_are_usage_errors() {
    let dir = fixture();
    std::fs::write(dir.path().join("urls.urls"), "# no network\n").unwrap();
    let output = invoke(dir.path(), &["urls.urls", "-o", "out", "--max-depth", "1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!dir.path().join("out").exists());
}

#[test]
fn explicit_history_opt_out_overrides_unsupported_environment_request() {
    let dir = fixture();
    let output = invoke_env(
        dir.path(),
        &["note.txt"],
        &[("MARKITAI_RECORD_HISTORY", "1")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("history.record"));
    let output = invoke_env(
        dir.path(),
        &["note.txt", "--no-record-history", "--pure"],
        &[("MARKITAI_RECORD_HISTORY", "1")],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("# Contract"));
}
