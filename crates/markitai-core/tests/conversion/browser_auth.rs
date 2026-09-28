use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Site {
    url: String,
    protected: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Option<String>>>>,
    stop: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl Site {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/article", listener.local_addr().unwrap());
        let protected = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (guard, seen, done) = (protected.clone(), requests.clone(), stop.clone());
        let task = std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("loopback accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut chunk = [0; 4096];
                    let Ok(count) = stream.read(&mut chunk) else {
                        break;
                    };
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    assert!(bytes.len() < 65536);
                }
                if bytes.is_empty() {
                    continue;
                }
                let request = String::from_utf8_lossy(&bytes);
                let auth = request.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_owned())
                });
                seen.lock().unwrap().push(auth.clone());
                let identity = match auth.as_deref() {
                    Some("Basic YWxpY2U6dGVzdC1vbmx5") => Some("Alice"),
                    Some("Basic Ym9iOnRlc3Qtb25seQ==") => Some("Bob"),
                    _ => None,
                };
                let locked = guard.load(Ordering::Acquire);
                let (status, header, text) = if !locked {
                    ("200 OK", "", "Anonymous cached article content".to_owned())
                } else if let Some(identity) = identity {
                    (
                        "200 OK",
                        "",
                        format!("Private article content for {identity}"),
                    )
                } else {
                    (
                        "401 Unauthorized",
                        "WWW-Authenticate: Basic realm=\"test\"\r\n",
                        "Authentication required".to_owned(),
                    )
                };
                let body = format!(
                    "<!doctype html><html><head><title>Article</title></head><body><article><h1>Article</h1><p>{text}</p><p>This independently authored page verifies the public conversion workflow and account-specific content.</p></article></body></html>"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\n{header}Connection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            url,
            protected,
            requests,
            stop,
            task: Some(task),
        }
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.task.take().unwrap().join().unwrap();
    }
}

fn isolated(selector: &str) -> bool {
    if std::env::var("MARKITAI_AUTH_API_TEST").as_deref() != Ok(selector) {
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", selector, "--ignored", "--nocapture"])
            .env_clear()
            .env("MARKITAI_AUTH_API_TEST", selector)
            .env("MARKITAI_HOME", dir.path().join("state"))
            .current_dir(dir.path())
            .stdout(std::process::Stdio::from(
                std::fs::File::create(dir.path().join("stdout")).unwrap(),
            ))
            .stderr(std::process::Stdio::from(
                std::fs::File::create(dir.path().join("stderr")).unwrap(),
            ));
        for name in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "SYSTEMROOT",
            "WINDIR",
            "MARKITAI_BROWSER_EXECUTABLE",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("auth API process timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let stdout = std::fs::read_to_string(dir.path().join("stdout")).unwrap();
        let stderr = std::fs::read_to_string(dir.path().join("stderr")).unwrap();
        assert!(status.success(), "{status}: {stdout}\n{stderr}");
        assert!(stdout.contains("1 passed"));
        return true;
    }
    false
}

#[test]
#[ignore = "Requires installed Chromium; run explicitly for authenticated public API acceptance"]
fn auto_auth_bypasses_anonymous_cache_and_keeps_accounts_separate() {
    if isolated("browser_auth::auto_auth_bypasses_anonymous_cache_and_keeps_accounts_separate") {
        return;
    }
    assert!(
        markitai_core::browser_available(),
        "requires installed Chromium"
    );
    let dir = tempfile::tempdir().unwrap();
    let site = Site::new();
    let cfg = json!({"llm":{"enabled":false},"fetch":{"strategy":"auto","remote_consent":"never","playwright":{"timeout":10000,"wait_for":"domcontentloaded"}},"image":{"alt_enabled":false,"desc_enabled":false}});
    let run = |config: Value, output| {
        convert(
            &site.url,
            ConvertOptions {
                config: Some(config),
                output_dir: output,
                ..Default::default()
            },
        )
    };
    let anonymous = run(cfg.clone(), None).unwrap();
    assert!(
        anonymous
            .markdown
            .contains("Anonymous cached article content")
    );
    assert_eq!(site.requests.lock().unwrap().len(), 1);
    site.protected.store(true, Ordering::Release);
    for user in ["alice", "bob"] {
        let mut authenticated = cfg.clone();
        authenticated["fetch"]["playwright"]["http_credentials"] =
            json!({"username":user,"password":"test-only"});
        authenticated["screenshot"] = json!({"enabled":true});
        let result = run(authenticated, Some(dir.path().join(user))).unwrap();
        let expected = if user == "alice" { "Alice" } else { "Bob" };
        assert!(
            result
                .markdown
                .contains(&format!("Private article content for {expected}"))
        );
        assert!(!result.markdown.contains("Anonymous cached article content"));
        assert!(!result.fetch_cache_hit());
        assert_eq!(result.fetch_strategy(), Some("playwright"));
        assert!(!result.screenshots.is_empty());
        for shot in &result.screenshots {
            assert!(shot.is_file());
            assert_eq!(
                image::guess_format(&std::fs::read(shot).unwrap()).unwrap(),
                image::ImageFormat::Jpeg
            );
        }
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("test-only") && !serialized.contains("dGVzdC1vbmx5"));
    }
    let count = site.requests.lock().unwrap().len();
    let still_anonymous = run(cfg.clone(), None).unwrap();
    assert!(still_anonymous.fetch_cache_hit());
    assert!(
        still_anonymous
            .markdown
            .contains("Anonymous cached article content")
    );
    assert_eq!(site.requests.lock().unwrap().len(), count);
    let mut explicit_static = cfg;
    explicit_static["fetch"]["strategy"] = json!("static");
    explicit_static["cache"]["enabled"] = json!(false);
    explicit_static["fetch"]["playwright"]["http_credentials"] =
        json!({"username":"alice","password":"test-only"});
    assert!(run(explicit_static, None).is_err());
    let observed = site.requests.lock().unwrap();
    assert_eq!(observed.len(), count + 1);
    assert!(
        observed.last().unwrap().is_none(),
        "static must not consume browser credentials"
    );
}

#[test]
#[ignore = "Requires installed Chromium; run explicitly for visual-to-text fallback acceptance"]
fn web_visual_validation_falls_back_to_typed_text_with_shared_usage_but_auth_does_not() {
    if isolated(
        "browser_auth::web_visual_validation_falls_back_to_typed_text_with_shared_usage_but_auth_does_not",
    ) {
        return;
    }
    use super::vision_processing::{Server, cfg, reply, typed};
    assert!(markitai_core::browser_available());
    let dir = tempfile::tempdir().unwrap();
    let site = Site::new();
    let model = Server::new(|request, _| {
        let system = request["messages"][0]["content"].as_str().unwrap();
        if system.contains("MARKITAI_VISION_JSON_V1") {
            (200, reply("Not a structured answer"))
        } else {
            assert!(system.contains("MARKITAI_DOCUMENT_JSON_V1"));
            (
                200,
                typed(
                    request["messages"][1]["content"].as_str().unwrap(),
                    "Recovered through text",
                ),
            )
        }
    });
    let settings = |model: &Server| {
        let mut config = cfg(model, dir.path());
        config["fetch"]["strategy"] = json!("playwright");
        config["fetch"]["playwright"] = json!({"timeout":10000,"wait_for":"domcontentloaded"});
        config["screenshot"] = json!({"enabled":true});
        config
    };
    let output = convert(
        &site.url,
        ConvertOptions {
            config: Some(settings(&model)),
            output_dir: Some(dir.path().join("recovered")),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(model.count(), 4);
    assert_eq!(output.usage.requests, 4);
    assert_eq!(output.usage.input_tokens, 28);
    assert_eq!(output.frontmatter["description"], "Recovered through text");
    assert!(
        output
            .llm_markdown
            .unwrap()
            .contains("Anonymous cached article content")
    );
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("structured text processing"))
    );
    let forbidden = Server::new(|_, _| {
        (
            401,
            json!({"error":{"message":"Denied"},"usage":{"prompt_tokens":3,"completion_tokens":1}}),
        )
    });
    let failed = convert(
        &site.url,
        ConvertOptions {
            config: Some(settings(&forbidden)),
            output_dir: Some(dir.path().join("denied")),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        forbidden.count(),
        1,
        "warnings={:?}; usage={:?}; enhanced={:?}",
        failed.warnings,
        failed.usage,
        failed.llm_markdown
    );
    assert_eq!(failed.usage.requests, 1);
    assert_eq!(failed.usage.input_tokens, 3);
    assert_eq!(failed.usage.output_tokens, 1);
    assert!(failed.llm_markdown.is_none());
    assert!(
        !failed
            .warnings
            .iter()
            .any(|warning| warning.contains("structured text processing"))
    );
}
