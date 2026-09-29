use markitai_core::{ConvertContext, ConvertOptions, convert_with_context, spa_domains};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn config(root: &Path) -> Value {
    json!({"llm":{"enabled":false},"ocr":{"enabled":false},"screenshot":{"enabled":false},
        "image":{"alt_enabled":false,"desc_enabled":false},"prompts":{"dir":root.join("prompts")},
        "fetch":{"strategy":"auto","playwright":{"timeout":5000,"extra_wait_ms":0,"skip_auto_scroll":true}},
        "cache":{"enabled":false,"no_cache":true,"no_cache_patterns":["*"],"global_dir":root.join("cache")}})
}

#[test]
fn public_management_of_missing_store_does_not_create_it() {
    let temporary = tempfile::tempdir().unwrap();
    let cfg = config(temporary.path());
    assert!(spa_domains::list(&cfg).unwrap().is_empty());
    spa_domains::preflight_clear(&cfg).unwrap();
    assert_eq!(spa_domains::clear(&cfg).unwrap(), 0);
    assert!(!temporary.path().join("cache").exists());
}

#[test]
#[ignore = "private subprocess helper; requires a supplied request"]
fn browser_conversion_child() {
    let Some(request_path) = std::env::var_os("MARKITAI_SPA_TEST_REQUEST") else {
        return;
    };
    let request: Value = serde_json::from_slice(&std::fs::read(request_path).unwrap()).unwrap();
    let options = ConvertOptions {
        config: Some(request["config"].clone()),
        ..Default::default()
    };
    let result = convert_with_context(
        request["source"].as_str().unwrap(),
        options,
        ConvertContext {
            explicit_fetch_strategy: request["explicit"].as_str(),
            ..Default::default()
        },
    );
    let result = match result {
        Ok(output) => json!({"ok":true,"cache_hit":output.fetch_cache_hit(),"output":output}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    std::fs::write(
        request["result"].as_str().unwrap(),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}

fn child(root: &Path, label: &str, source: &str, cfg: &Value, explicit: Option<&str>) -> Value {
    let request = root.join(format!("{label}.request.json"));
    let result = root.join(format!("{label}.result.json"));
    let stdout = root.join(format!("{label}.stdout"));
    let stderr = root.join(format!("{label}.stderr"));
    std::fs::write(
        &request,
        serde_json::to_vec(
            &json!({"source":source,"config":cfg,"explicit":explicit,"result":result}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "browser_conversion_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("MARKITAI_SPA_TEST_REQUEST", &request)
        .env("MARKITAI_HOME", root.join("home"))
        .current_dir(root)
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(&stderr).unwrap());
    for name in [
        "HOME",
        "PATH",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "TMPDIR",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "TZ",
        "MARKITAI_BROWSER_EXECUTABLE",
        "PLAYWRIGHT_BROWSERS_PATH",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("browser learning subprocess timed out: {label}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "{label}: {}",
        std::fs::read_to_string(stderr).unwrap()
    );
    assert!(
        std::fs::read_to_string(stdout)
            .unwrap()
            .contains("1 passed")
    );
    serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap()
}

#[derive(Clone)]
struct Request {
    path: String,
    browser: bool,
    cookie: String,
}
struct Site {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl Site {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (requests.clone(), stop.clone());
        let task = std::thread::spawn(move || {
            let mut active = Vec::new();
            while !stopped.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("learning loopback accept failed: {error}"),
                };
                let seen = seen.clone();
                active.push(std::thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request_reader = bounded_fixture_io::Reader::new(&stream, std::time::Instant::now() + Duration::from_secs(2));
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes = Vec::new();
                    while !bytes.windows(4).any(|value| value == b"\r\n\r\n") {
                        let mut buffer = [0u8; 4096];
                        let Ok(n) = request_reader.read(&mut buffer) else { return };
                        if n == 0 { return }
                        bytes.extend_from_slice(&buffer[..n]);
                        assert!(bytes.len() <= 32 * 1024);
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    let path = text.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
                    let header = |name: &str| text.lines().find_map(|line| { let (key, value) = line.split_once(':')?; key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned()) }).unwrap_or_default();
                    let browser = !header("User-Agent").starts_with("markitai/");
                    seen.lock().unwrap().push(Request { path: path.clone(), browser, cookie: header("Cookie") });
                    let body = match path.split('?').next().unwrap() {
                        "/article" => "<article><h1>Legitimate article</h1><p>Please enable JavaScript is an example quoted in this complete technical tutorial.</p></article>",
                        "/plain" => "Please enable JavaScript is a literal text document.",
                        "/empty" => "<html><body></body></html>",
                        "/challenge" => "<html><title>Just a moment</title><body><div id='cf-chl-widget'>Verify you are human</div></body></html>",
                        _ => "<html><head><title>Browser learning fixture</title></head><body>Please enable JavaScript<script>document.body.innerHTML='<article><h1>Rendered learning result</h1><p>Every request retrieves this meaningful visible paragraph from the private loopback fixture.</p></article>';</script></body></html>",
                    };
                    let status = if path.starts_with("/failed") && browser { "503 Service Unavailable" } else { "200 OK" };
                    let mime = if path.starts_with("/plain") { "text/plain" } else { "text/html" };
                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes());
                }));
            }
            for thread in active {
                thread.join().unwrap();
            }
        });
        Self {
            base,
            requests,
            stop,
            task: Some(task),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    fn count(&self, path: &str, browser: bool) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path == path && request.browser == browser)
            .count()
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let joined = task.join();
            if !std::thread::panicking() {
                joined.unwrap();
            }
        }
    }
}

#[test]
#[ignore = "requires installed Chromium; fresh private subprocesses and loopback only"]
fn learned_routes_cross_processes_but_not_explicit_intents_or_browser_identities() {
    assert!(
        markitai_core::browser_available(),
        "set MARKITAI_BROWSER_EXECUTABLE for this selected test"
    );
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(temp.path());
    let site = Site::new();
    for (label, expected_static) in [("first", 1), ("second", 0)] {
        let path = format!("/{label}");
        let output = child(temp.path(), label, &site.url(&path), &cfg, None);
        assert_eq!(output["ok"], true, "{output}");
        assert_eq!(output["cache_hit"], false);
        assert!(
            output["output"]["markdown"]
                .as_str()
                .unwrap()
                .contains("meaningful visible paragraph")
        );
        assert_eq!(site.count(&path, false), expected_static);
        assert_eq!(site.count(&path, true), 1);
    }
    assert_eq!(spa_domains::list(&cfg).unwrap()[0].hits, 2);
    let mut explicit_static = cfg.clone();
    explicit_static["fetch"]["strategy"] = json!("static");
    assert_eq!(
        child(
            temp.path(),
            "static",
            &site.url("/static"),
            &explicit_static,
            Some("static")
        )["ok"],
        false
    );
    assert_eq!(
        (site.count("/static", false), site.count("/static", true)),
        (1, 0)
    );
    assert_eq!(
        child(
            temp.path(),
            "explicit-auto",
            &site.url("/explicit-auto"),
            &cfg,
            Some("auto")
        )["ok"],
        true
    );
    assert_eq!(
        (
            site.count("/explicit-auto", false),
            site.count("/explicit-auto", true)
        ),
        (1, 1)
    );
    let hits = spa_domains::list(&cfg).unwrap()[0].hits;
    for identity in ["accountA", "accountB"] {
        let mut private = cfg.clone();
        private["fetch"]["playwright"]["cookies"] = json!([{"name":"account","value":identity}]);
        let path = format!("/{identity}");
        assert_eq!(
            child(temp.path(), identity, &site.url(&path), &private, None)["ok"],
            true
        );
        assert_eq!((site.count(&path, false), site.count(&path, true)), (0, 1));
        assert!(
            site.requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.path == path
                    && request.cookie == format!("account={identity}"))
        );
    }
    assert_eq!(spa_domains::list(&cfg).unwrap()[0].hits, hits);
    let other_port = Site::new();
    assert_eq!(
        child(
            temp.path(),
            "other-port",
            &other_port.url("/other"),
            &cfg,
            None
        )["ok"],
        true
    );
    assert_eq!(
        (
            other_port.count("/other", false),
            other_port.count("/other", true)
        ),
        (1, 1)
    );
    assert_eq!(spa_domains::clear(&cfg).unwrap(), 2);
    assert_eq!(
        child(
            temp.path(),
            "after-clear",
            &site.url("/after-clear"),
            &cfg,
            None
        )["ok"],
        true
    );
    assert_eq!(
        (
            site.count("/after-clear", false),
            site.count("/after-clear", true)
        ),
        (1, 1)
    );
}

#[test]
#[ignore = "requires installed Chromium; fresh private subprocesses and loopback only"]
fn invalid_browser_results_do_not_teach_routes_and_corrupt_store_is_only_a_warning() {
    assert!(markitai_core::browser_available());
    let temp = tempfile::tempdir().unwrap();
    let cfg = config(temp.path());
    let site = Site::new();
    for name in ["article", "plain", "empty", "challenge", "failed"] {
        let result = child(
            temp.path(),
            name,
            &site.url(&format!("/{name}")),
            &cfg,
            None,
        );
        if matches!(name, "article" | "plain") {
            assert_eq!(result["ok"], true, "{result}");
        }
        if matches!(name, "empty" | "failed") {
            assert_eq!(result["ok"], false, "{result}");
        }
        assert!(
            spa_domains::list(&cfg).unwrap().is_empty(),
            "incorrect learning from {name}"
        );
    }
    let signed = child(
        temp.path(),
        "signed",
        &site.url("/signed?token=private-fixture"),
        &cfg,
        None,
    );
    assert_eq!(signed["ok"], true);
    assert!(spa_domains::list(&cfg).unwrap().is_empty());
    assert_eq!(
        child(temp.path(), "train", &site.url("/train"), &cfg, None)["ok"],
        true
    );
    let store = temp.path().join("cache/learned_spa_domains.db");
    std::fs::write(&store, b"private corrupt database content").unwrap();
    let result = child(temp.path(), "corrupt", &site.url("/corrupt"), &cfg, None);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["cache_hit"], false);
    assert_eq!(
        (site.count("/corrupt", false), site.count("/corrupt", true)),
        (1, 1)
    );
    let warnings = result["output"]["warnings"].as_array().unwrap();
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning
                .as_str()
                .unwrap()
                .contains("Learned browser-domain store is unavailable"))
            .count(),
        1
    );
    assert!(
        !serde_json::to_string(warnings)
            .unwrap()
            .contains("private corrupt")
    );
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
