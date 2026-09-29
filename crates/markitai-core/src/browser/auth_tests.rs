//! These optional tests launch the installed browser against authored loopback pages.
use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

const USER: &str = "fixture-user";
const PASSWORD: &str = "fixture-private-password";

#[derive(Clone)]
struct Request {
    path: String,
    authorization: Option<String>,
}
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}
impl Reply {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
            body: body.into(),
        }
    }
    fn challenge() -> Self {
        Self {
            status: 401,
            headers: vec![("WWW-Authenticate".into(), "Basic realm=\"fixture\"".into())],
            body: "Authentication required".into(),
        }
    }
    fn redirect(url: &str) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".into(), url.into())],
            body: String::new(),
        }
    }
}
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = requests.clone();
        let stopping = stop.clone();
        let handler = Arc::new(handler);
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let handler = handler.clone();
                        let recorded = recorded.clone();
                        workers.push(thread::spawn(move || {
                            if let Some(request)=read_request(&stream) {
                                let reply=handler(&request);recorded.lock().unwrap().push(request);
                                let mut bytes=format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Length: {}\r\n",reply.status,reply.body.len());
                                for (key,value) in reply.headers { bytes.push_str(&format!("{key}: {value}\r\n")); }
                                bytes.push_str("\r\n");bytes.push_str(&reply.body);
                                let mut stream=stream;let _=stream.write_all(bytes.as_bytes());
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("loopback listener failed: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            origin,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
    fn recorded(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
fn read_request(stream: &TcpStream) -> Option<Request> {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request_reader =
        bounded_fixture_io::Reader::new(stream, std::time::Instant::now() + Duration::from_secs(2));
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        let read = request_reader.read(&mut buffer).ok()?;
        if read == 0 || bytes.len() + read > 32 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    Some(Request {
        path: text.lines().next()?.split_whitespace().nth(1)?.into(),
        authorization: text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        }),
    })
}
fn expected_header() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"))
    )
}
fn config() -> Value {
    json!({"fetch":{"playwright":{"timeout":8000,"wait_for":"load","extra_wait_ms":0,"skip_auto_scroll":true,"http_credentials":{"username":USER,"password":PASSWORD}}},"screenshot":{"viewport_width":320,"viewport_height":240,"tile_height":0}})
}
fn installed() {
    assert!(
        available(),
        "this explicitly selected test requires an installed Chromium executable"
    );
}

#[test]
#[ignore = "requires an installed Chromium; uses only isolated loopback servers"]
fn installed_chromium_basic_auth_captures_authenticated_content_without_preemptive_secret() {
    installed();
    let expected = expected_header();
    let server = Server::new(move |request| {
        if request.authorization.as_deref() == Some(&expected) {
            Reply::ok(
                "<!doctype html><html><head><title>Authenticated fixture</title></head><body style='margin:0;background:rgb(40,90,160);min-height:240px'><h1>AUTHENTICATED CONTENT</h1></body></html>",
            )
        } else {
            Reply::challenge()
        }
    });
    let page = fetch(&server.url("/document"), &config(), true)
        .unwrap()
        .page();
    assert!(page.html.contains("AUTHENTICATED CONTENT"));
    assert_eq!(page.title, "Authenticated fixture");
    assert_eq!(page.final_url, server.url("/document"));
    assert!(!page.html.contains(PASSWORD));
    assert_eq!(page.screenshots.len(), 1);
    let image =
        image::load_from_memory_with_format(&page.screenshots[0].bytes, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8();
    assert_eq!(image.width(), 320);
    let pixel = image.get_pixel(5, 200).0;
    assert!(
        (30..=50).contains(&pixel[0])
            && (80..=100).contains(&pixel[1])
            && (150..=170).contains(&pixel[2])
    );
    let requests = server.recorded();
    let requests = requests
        .iter()
        .filter(|r| r.path == "/document")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].authorization.is_none());
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some(expected_header().as_str())
    );
}

#[test]
#[ignore = "requires an installed Chromium; uses only isolated loopback servers"]
fn installed_chromium_wrong_password_stops_after_one_credential_attempt() {
    installed();
    let server = Server::new(|_| Reply::challenge());
    let error = fetch(&server.url("/denied"), &config(), false)
        .err()
        .expect("wrong credentials must not yield content")
        .to_string();
    assert!(
        !error.contains(USER) && !error.contains(PASSWORD) && !error.contains(&expected_header())
    );
    let requests = server.recorded();
    let requests = requests
        .iter()
        .filter(|r| r.path == "/denied")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].authorization.is_none());
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.authorization.is_some())
            .count(),
        1
    );
}

#[test]
#[ignore = "requires an installed Chromium; uses only isolated loopback servers"]
fn installed_chromium_redirect_auth_requires_the_explicit_target_origin() {
    installed();
    let expected = expected_header();
    let target = Server::new(move |request| {
        if request.authorization.as_deref() == Some(&expected) {
            Reply::ok("<h1>EXPLICIT ORIGIN CONTENT</h1>")
        } else {
            Reply::challenge()
        }
    });
    let destination = target.url("/protected");
    let source = Server::new(move |_| Reply::redirect(&destination));
    assert!(fetch(&source.url("/start"), &config(), false).is_err());
    assert!(target.recorded().iter().all(|r| r.authorization.is_none()));
    let mut cfg = config();
    cfg["fetch"]["playwright"]["http_credentials"]["origin"] = json!(target.origin);
    let page = fetch(&source.url("/start"), &cfg, false).unwrap().page();
    assert!(page.html.contains("EXPLICIT ORIGIN CONTENT"));
    assert_eq!(page.final_url, target.url("/protected"));
    assert!(source.recorded().iter().all(|r| r.authorization.is_none()));
    assert!(
        target
            .recorded()
            .iter()
            .any(|r| r.authorization.as_deref() == Some(expected_header().as_str()))
    );
}

#[test]
#[ignore = "requires an installed Chromium; uses only isolated loopback servers"]
fn installed_chromium_foreign_subresource_challenge_does_not_receive_page_password() {
    installed();
    let foreign = Server::new(|_| Reply::challenge());
    let image = foreign.url("/private-image");
    let expected = expected_header();
    let source = Server::new(move |request| {
        if request.authorization.as_deref() == Some(&expected) {
            Reply::ok(&format!("<h1>PRIVATE MAIN PAGE</h1><img src='{image}'>"))
        } else {
            Reply::challenge()
        }
    });
    let page = fetch(&source.url("/document"), &config(), false)
        .unwrap()
        .page();
    assert!(page.html.contains("PRIVATE MAIN PAGE"));
    let requests = foreign.recorded();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|r| r.authorization.is_none()));
}

#[test]
#[ignore = "requires an installed Chromium; launches only a private about:blank page"]
fn installed_chromium_diagnostic_and_session_cleanup_are_real() {
    installed();
    let executable = discover().unwrap();
    let browser = cdp::Browser::launch(&executable, &options::Options::diagnostic()).unwrap();
    let (pid, profile) = browser.test_process().unwrap();
    assert!(profile.is_dir());
    drop(browser);
    assert!(!profile.exists());
    #[cfg(unix)]
    assert_eq!(
        unsafe { libc::kill(pid as i32, 0) },
        -1,
        "browser child must have been reaped"
    );
    #[cfg(not(unix))]
    let _ = pid;
    assert_eq!(diagnostic().unwrap(), Some(executable));
}

#[test]
fn diagnostic_failure_does_not_expose_the_selected_path() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory
        .path()
        .join("private-browser-path-that-does-not-exist");
    let error = diagnostic_with(&selected).unwrap_err().to_string();
    assert_eq!(
        error,
        "Chromium diagnostic could not initialize a private blank page"
    );
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
