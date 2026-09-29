use super::*;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const PDF: &[u8] = include_bytes!("../../src/pdf_raster/fixtures/mixed-native-scanned-blank.pdf");
const AUTH: &str = "Basic YWxpY2U6dGVzdC1vbmx5";

// Each optional browser acceptance test runs in a clean process, preserving HOME.
fn isolated(name: &str) -> bool {
    let selector = format!("browser_pdf::{name}");
    if std::env::var("MARKITAI_BROWSER_PDF_API_TEST").as_deref() == Ok(selector.as_str()) {
        return false;
    }
    let dir = tempfile::tempdir().unwrap();
    let temporary = dir.path().join("tmp");
    std::fs::create_dir(&temporary).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &selector, "--ignored", "--nocapture"])
        .env_clear()
        .env("MARKITAI_BROWSER_PDF_API_TEST", &selector)
        .env("MARKITAI_HOME", dir.path().join("state"))
        .env("TMPDIR", temporary)
        .current_dir(dir.path())
        .stdout(std::process::Stdio::from(
            std::fs::File::create(dir.path().join("stdout")).unwrap(),
        ))
        .stderr(std::process::Stdio::from(
            std::fs::File::create(dir.path().join("stderr")).unwrap(),
        ));
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "LANG",
        "LC_ALL",
        "TZ",
        "MARKITAI_BROWSER_EXECUTABLE",
        "PLAYWRIGHT_BROWSERS_PATH",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("browser PDF API test timed out: {selector}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(dir.path().join("stdout")).unwrap();
    let stderr = std::fs::read_to_string(dir.path().join("stderr")).unwrap();
    assert!(status.success(), "{selector}: {status}\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("1 passed"),
        "selector did not execute: {stdout}"
    );
    true
}

#[derive(Clone)]
struct Request {
    path: String,
    auth: Option<String>,
}
struct Site {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Site {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("loopback accept failed: {e}"),
                };
                let seen = seen.clone();
                connections.push(std::thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes = Vec::new();
                    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        let mut chunk = [0; 4096];
                        let Ok(count) = stream.read(&mut chunk) else { return; };
                        if count == 0 { return; }
                        bytes.extend_from_slice(&chunk[..count]);
                        assert!(bytes.len() <= 32 * 1024);
                    }
                    let header = std::str::from_utf8(&bytes).unwrap();
                    let path = header.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
                    let auth = header.lines().find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("authorization")).map(|(_, value)| value.trim().to_owned()));
                    seen.lock().unwrap().push(Request { path: path.clone(), auth: auth.clone() });
                    let (status, headers, body): (&str, &str, &[u8]) = if path.starts_with("/start") {
                        ("302 Found", "Location: /download?token=final-secret\r\n", b"")
                    } else if auth.as_deref() == Some(AUTH) && path.starts_with("/html") {
                        ("200 OK", "Content-Type: text/html\r\n", b"<html><body><article><p>Private authored HTML content requiring an observable screenshot output.</p></article></body></html>")
                    } else if auth.as_deref() == Some(AUTH) {
                        ("200 OK", "Content-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"original.pdf\"\r\n", PDF)
                    } else {
                        ("401 Unauthorized", "Content-Type: text/plain\r\nWWW-Authenticate: Basic realm=\"private-pdf\"\r\n", b"Authentication required")
                    };
                    let _ = write!(stream, "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(body);
                }));
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            base,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    fn assert_one_body(&self) {
        let requests = self.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.starts_with("/download")
                    && request.auth.as_deref() == Some(AUTH))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.starts_with("/download") && request.auth.is_none())
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .all(|request| request.auth.as_deref().is_none_or(|value| value == AUTH))
        );
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}
fn cfg(root: &Path) -> Value {
    json!({"history":{"record":false},"prompts":{"dir":root.join("prompts")},"cache":{"enabled":false,"global_dir":root.join("cache")},"llm":{"enabled":false},"ocr":{"enabled":false},"image":{"compress":false,"format":"png","alt_enabled":false,"desc_enabled":false},"fetch":{"strategy":"auto","no_remote":true,"playwright":{"timeout":10000,"wait_for":"load","skip_auto_scroll":true,"extra_wait_ms":0,"http_credentials":{"username":"alice","password":"test-only"}}}})
}

#[test]
#[ignore = "Requires installed Chromium; authenticated HTML/PDF classification"]
fn browser_memory_capture_classifies_before_enforcing_html_output_requirement() {
    if isolated("browser_memory_capture_classifies_before_enforcing_html_output_requirement") {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let site = Site::new();
    let mut config = cfg(root.path());
    config["fetch"]["strategy"] = json!("playwright");
    config["screenshot"] = json!({"screenshot_only":true});
    let document = convert(
        &site.url("/download"),
        ConvertOptions {
            config: Some(config.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(document.markdown.contains("NATIVE PAGE ONE"));
    assert!(document.output_path.is_none() && document.screenshots.is_empty());
    site.assert_one_body();
    let error = convert(
        &site.url("/html"),
        ConvertOptions {
            config: Some(config),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(error.to_string().contains("output_dir"));
    assert!(
        site.requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == "/html" && request.auth.as_deref() == Some(AUTH))
    );
}

#[test]
#[ignore = "Requires installed Chromium; authenticated PDF public API acceptance"]
fn authenticated_redirect_download_uses_native_pdf_and_preserves_source() {
    if isolated("authenticated_redirect_download_uses_native_pdf_and_preserves_source") {
        return;
    }
    assert!(markitai_core::browser_available());
    let root = tempfile::tempdir().unwrap();
    let site = Site::new();
    let source = site.url("/start?token=source-secret");
    let result = convert(
        &source,
        ConvertOptions {
            config: Some(cfg(root.path())),
            output_dir: Some(root.path().join("output")),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.source, source);
    assert_eq!(
        result.frontmatter["source"],
        site.url("/start?token=REDACTED")
    );
    assert_eq!(
        result.frontmatter["source_url"],
        site.url("/download?token=REDACTED")
    );
    assert_eq!(result.fetch_strategy(), Some("playwright"));
    assert!(!result.fetch_cache_hit());
    assert!(result.markdown.contains("NATIVE PAGE ONE"));
    assert_eq!(result.markdown.matches("<!-- Page number:").count(), 3);
    assert!(
        std::fs::read_to_string(result.output_path.as_ref().unwrap())
            .unwrap()
            .contains("NATIVE PAGE ONE")
    );
    assert!(result.screenshots.is_empty());
    site.assert_one_body();
    assert_eq!(
        site.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path.starts_with("/start"))
            .count(),
        1
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires installed Chromium and macOS PDF rendering; loopback vision only"]
fn authenticated_pdf_memory_vision_receives_all_native_png_pages() {
    use base64::Engine;
    if isolated("authenticated_pdf_memory_vision_receives_all_native_png_pages") {
        return;
    }
    assert!(markitai_core::browser_available());
    let root = tempfile::tempdir().unwrap();
    let site = Site::new();
    let model = super::vision_processing::Server::new(|request, _| {
        let content = request["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            4,
            "all three native pages accompany the protected body"
        );
        let body = content[0]["text"].as_str().unwrap();
        assert!(body.contains("NATIVE PAGE ONE"));
        for block in &content[1..] {
            let encoded = block["image_url"]["url"]
                .as_str()
                .unwrap()
                .strip_prefix("data:image/png;base64,")
                .unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap();
            assert_eq!(
                image::guess_format(&bytes).unwrap(),
                image::ImageFormat::Png
            );
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (1275, 1650));
        }
        (
            200,
            super::vision_processing::typed(body, "Authenticated PDF description"),
        )
    });
    let mut config = cfg(root.path());
    config["fetch"]["strategy"] = json!("playwright");
    config["screenshot"] = json!({"enabled":true});
    config["llm"] = json!({"enabled":true,"keep_base":true,"on_failure":"fail","router_settings":{"timeout":10,"num_retries":0},"model_list":[{"model_name":"download-vision","litellm_params":{"model":"openai/fixture","api_key":"local-fixture","api_base":model.base},"model_info":{"supports_vision":true}}]});
    let result = convert(
        &site.url("/download"),
        ConvertOptions {
            config: Some(config),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.fetch_strategy(), Some("playwright"));
    assert!(!result.fetch_cache_hit());
    assert!(result.markdown.contains("NATIVE PAGE ONE"));
    assert_eq!(result.markdown.matches("<!-- Page number:").count(), 3);
    assert!(
        result
            .llm_markdown
            .as_ref()
            .unwrap()
            .contains("NATIVE PAGE ONE")
    );
    assert!(result.output_path.is_none() && result.screenshots.is_empty());
    assert_eq!(model.count(), 1);
    site.assert_one_body();
}
