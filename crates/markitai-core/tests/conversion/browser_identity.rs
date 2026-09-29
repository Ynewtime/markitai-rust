use super::*;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

// Each optional browser acceptance test runs in a clean process, preserving HOME.
fn isolated(name: &str) -> bool {
    let selector = format!("browser_identity::{name}");
    if std::env::var("MARKITAI_BROWSER_IDENTITY_TEST").as_deref() == Ok(selector.as_str()) {
        return false;
    }
    let dir = tempfile::tempdir().unwrap();
    let temporary = dir.path().join("tmp");
    std::fs::create_dir(&temporary).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &selector, "--ignored", "--nocapture"])
        .env_clear()
        .env("MARKITAI_BROWSER_IDENTITY_TEST", &selector)
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
            panic!("browser identity API test timed out: {selector}");
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

#[derive(Clone, Copy)]
enum Identity {
    Cookie,
    Header,
}
struct Request {
    path: String,
    cookie: Option<String>,
    authorization: Option<String>,
}
struct Site {
    base: String,
    locked: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Site {
    fn new(identity: Identity) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let locked = Arc::new(AtomicBool::new(false));
        let protected = locked.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("identity loopback accept: {error}"),
                };
                let protected = protected.clone();
                let observed = observed.clone();
                connections.push(std::thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request_reader = bounded_fixture_io::Reader::new(&stream, std::time::Instant::now() + Duration::from_secs(2));
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes=Vec::new();
                    while !bytes.windows(4).any(|value|value==b"\r\n\r\n") {
                        let mut buffer=[0u8;4096];let Ok(n)=request_reader.read(&mut buffer) else{return;};if n==0{return;}
                        bytes.extend_from_slice(&buffer[..n]);assert!(bytes.len()<=32*1024);
                    }
                    let text=String::from_utf8(bytes).unwrap();
                    let path=text.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
                    let header=|name:&str|text.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case(name)).map(|(_,value)|value.trim().to_owned()));
                    let cookie=header("cookie");let authorization=header("authorization");
                    let authenticated=match identity {
                        Identity::Cookie=>cookie.as_deref().is_some_and(|value|value.split(';').any(|item|item.trim()=="session=private-cookie")),
                        Identity::Header=>authorization.as_deref()==Some("Bearer private-header"),
                    };
                    observed.lock().unwrap().push(Request{path:path.clone(),cookie,authorization});
                    let (status,body)=if path!="/article" {("404 Not Found",String::new())}
                    else if protected.load(Ordering::Acquire) && !authenticated {("403 Forbidden","This request has no browser identity.".into())}
                    else {
                        let content=if authenticated {"PRIVATE IDENTITY CONTENT"} else {"ANONYMOUS CACHED CONTENT"};
                        ("200 OK",format!("<!doctype html><html><head><title>Identity article</title></head><body><article><h1>Identity article</h1><p>{content}</p><p>This complete authored article checks account-specific text and screenshot capture without any remote resource.</p></article></body></html>"))
                    };
                    // Deliberately no validators: the last anonymous result is
                    // a valid fresh cache hit without an additional HTTP call.
                    let _=write!(stream,"HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                }));
            }
            for worker in connections {
                worker.join().unwrap();
            }
        });
        Self {
            base,
            locked,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn article_requests(&self) -> (usize, usize) {
        let requests = self.requests.lock().unwrap();
        let mut anonymous = 0;
        let mut authenticated = 0;
        for request in requests.iter().filter(|request| request.path == "/article") {
            if request.cookie.is_none() && request.authorization.is_none() {
                anonymous += 1;
            } else {
                authenticated += 1;
            }
        }
        (anonymous, authenticated)
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let status = worker.join();
            if !std::thread::panicking() {
                status.unwrap();
            }
        }
    }
}
fn cfg(root: &Path) -> Value {
    json!({"history":{"record":false},"prompts":{"dir":root.join("prompts")},"cache":{"enabled":true,"global_dir":root.join("cache"),"fetch_ttl_seconds":3600},"llm":{"enabled":false},"ocr":{"enabled":false},"image":{"alt_enabled":false,"desc_enabled":false},"fetch":{"strategy":"auto","no_remote":true,"playwright":{"timeout":10000,"wait_for":"load","skip_auto_scroll":true,"extra_wait_ms":0}},"screenshot":{"enabled":false}})
}
fn check(identity: Identity) {
    assert!(markitai_core::browser_available());
    let dir = tempfile::tempdir().unwrap();
    let site = Site::new(identity);
    let url = format!("{}/article", site.base);
    let config = cfg(dir.path());
    let convert_with = |config: Value, output_dir| {
        convert(
            &url,
            ConvertOptions {
                config: Some(config),
                output_dir,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let anonymous = convert_with(config.clone(), None);
    assert!(!anonymous.fetch_cache_hit());
    assert!(anonymous.markdown.contains("ANONYMOUS CACHED CONTENT"));
    assert_eq!(site.article_requests(), (1, 0));
    site.locked.store(true, Ordering::Release);
    let mut identified = config.clone();
    match identity {
        Identity::Cookie => {
            identified["fetch"]["playwright"]["cookies"] =
                json!([{"name":"session","value":"private-cookie","url":url}])
        }
        Identity::Header => {
            identified["fetch"]["playwright"]["extra_http_headers"] =
                json!({"Authorization":"Bearer private-header"})
        }
    }
    // A fresh anonymous cache entry cannot satisfy a no-capture identity fetch.
    let private = convert_with(identified.clone(), None);
    assert!(!private.fetch_cache_hit());
    assert_eq!(private.fetch_strategy(), Some("playwright"));
    assert!(private.markdown.contains("PRIVATE IDENTITY CONTENT"));
    assert!(!private.markdown.contains("ANONYMOUS"));
    assert!(private.screenshots.is_empty());
    assert_eq!(private.source, url);
    assert_eq!(site.article_requests(), (1, 1));
    let mut captured = identified.clone();
    captured["screenshot"]["enabled"] = json!(true);
    let private = convert_with(captured, Some(dir.path().join("capture")));
    assert_eq!(private.fetch_strategy(), Some("playwright"));
    assert!(!private.fetch_cache_hit());
    assert!(private.markdown.contains("PRIVATE IDENTITY CONTENT"));
    assert!(!private.screenshots.is_empty());
    for image in &private.screenshots {
        let bytes = std::fs::read(image).unwrap();
        assert!(image::load_from_memory(&bytes).is_ok());
    }
    assert_eq!(
        site.article_requests(),
        (1, 2),
        "anonymous GET must not precede either private navigation"
    );
    // Neither identity fetch overwrites the anonymous cache row.
    let restored = convert_with(config, None);
    assert!(restored.fetch_cache_hit());
    assert_eq!(restored.markdown, anonymous.markdown);
    assert_eq!(site.article_requests(), (1, 2));
    // Explicit static retains its chosen anonymous representation even when
    // browser credentials are configured; it does not enter the identity branch.
    identified["fetch"]["strategy"] = json!("static");
    let explicit = convert_with(identified, None);
    assert!(explicit.fetch_cache_hit());
    assert_eq!(explicit.fetch_strategy(), Some("static"));
    assert_eq!(explicit.markdown, anonymous.markdown);
    assert_eq!(site.article_requests(), (1, 2));
}

#[test]
#[ignore = "Requires installed Chromium; isolated cookie identity and capture"]
fn cookie_only_identity_bypasses_anonymous_cache_and_probe() {
    if isolated("cookie_only_identity_bypasses_anonymous_cache_and_probe") {
        return;
    }
    check(Identity::Cookie);
}

#[test]
#[ignore = "Requires installed Chromium; isolated header identity and capture"]
fn authorization_header_identity_bypasses_anonymous_cache_and_probe() {
    if isolated("authorization_header_identity_bypasses_anonymous_cache_and_probe") {
        return;
    }
    check(Identity::Header);
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
