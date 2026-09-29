//! The server validates digest responses; Chrome alone produces client credentials.
use super::*;
use crate::BrowserRuntime;
use md5::{Digest as Md5Digest, Md5};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;

const USER: &str = "digest-fixture-user";
const PASSWORD: &str = "digest-fixture-password";
const REALM: &str = "private,digest realm";
const OPAQUE: &str = "opaque,fixture";
const FIRST: &str = "fixture-initial-nonce";
const SECOND: &str = "fixture-renewed-nonce";
const PDF: &[u8] = include_bytes!("../pdf_raster/fixtures/mixed-native-scanned-blank.pdf");

#[derive(Clone, Copy)]
enum Algorithm {
    Md5,
    Md5Sess,
    Sha256,
    Sha256Sess,
}
impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Md5Sess => "MD5-sess",
            Self::Sha256 => "SHA-256",
            Self::Sha256Sess => "SHA-256-sess",
        }
    }
    fn hash(self, input: &str) -> String {
        let bytes = match self {
            Self::Md5 | Self::Md5Sess => Md5::digest(input.as_bytes()).to_vec(),
            Self::Sha256 | Self::Sha256Sess => Sha256::digest(input.as_bytes()).to_vec(),
        };
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(&mut output, "{byte:02x}").unwrap();
        }
        output
    }
    fn response(self, fields: &BTreeMap<String, String>, method: &str, password: &str) -> String {
        let mut identity = self.hash(&format!(
            "{}:{}:{password}",
            fields["username"], fields["realm"]
        ));
        if matches!(self, Self::Md5Sess | Self::Sha256Sess) {
            identity = self.hash(&format!(
                "{identity}:{}:{}",
                fields["nonce"], fields["cnonce"]
            ));
        }
        let resource = self.hash(&format!("{method}:{}", fields["uri"]));
        self.hash(&format!(
            "{identity}:{}:{}:{}:auth:{resource}",
            fields["nonce"], fields["nc"], fields["cnonce"]
        ))
    }
    fn verify(self, request: &Request, nonce: &str, user: &str, password: &str) -> bool {
        let Some(fields) = request.authorization.as_deref().and_then(parameters) else {
            return false;
        };
        let field = |key: &str| fields.get(key).map(String::as_str);
        if field("username") != Some(user)
            || field("realm") != Some(REALM)
            || field("nonce") != Some(nonce)
            || field("opaque") != Some(OPAQUE)
            || field("uri") != Some(request.path.as_str())
            || field("qop") != Some("auth")
            || !field("algorithm").is_some_and(|value| value.eq_ignore_ascii_case(self.name()))
            || !field("nc").is_some_and(|value| {
                value.len() == 8
                    && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && u32::from_str_radix(value, 16).is_ok_and(|count| count > 0)
            })
            || !field("cnonce").is_some_and(|value| !value.is_empty() && value.len() <= 256)
        {
            return false;
        }
        let expected = self.response(&fields, &request.method, password);
        field("response") == Some(expected.as_str())
    }
}

// Quoted values can contain commas; duplicate parameters are never accepted.
fn parameters(value: &str) -> Option<BTreeMap<String, String>> {
    if value.len() > 16_384 || !value.is_ascii() {
        return None;
    }
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("digest") {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut index = 0;
    let mut output = BTreeMap::new();
    while index < bytes.len() {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            index += 1;
        }
        if index == start {
            return None;
        }
        let key = rest[start..index].to_ascii_lowercase();
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            return None;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let mut text = String::new();
        if bytes.get(index) == Some(&b'"') {
            index += 1;
            loop {
                let byte = *bytes.get(index)?;
                index += 1;
                if byte == b'"' {
                    break;
                }
                let byte = if byte == b'\\' {
                    let next = *bytes.get(index)?;
                    index += 1;
                    next
                } else {
                    byte
                };
                if byte.is_ascii_control() {
                    return None;
                }
                text.push(char::from(byte));
            }
        } else {
            let start = index;
            while bytes
                .get(index)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b',')
            {
                index += 1;
            }
            if start == index {
                return None;
            }
            text.push_str(&rest[start..index]);
        }
        if output.insert(key, text).is_some() || output.len() > 32 {
            return None;
        }
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        if bytes.get(index) != Some(&b',') {
            return None;
        }
        index += 1;
        if index == bytes.len() {
            return None;
        }
    }
    Some(output)
}

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    authorization: Option<String>,
}
#[derive(Clone)]
struct Recorded {
    request: Request,
    status: u16,
}
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn response(status: u16, mime: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), mime.into())],
            body,
        }
    }
    fn ok() -> Self {
        Self::response(200, "text/html; charset=utf-8", b"<!doctype html><title>Digest fixture</title><body style='margin:0;background:rgb(40,90,160);height:240px'><h1 style='margin:0'>AUTHENTICATED DIGEST BODY</h1></body>".to_vec())
    }
    fn challenge(algorithm: Algorithm, nonce: &str, stale: bool) -> Self {
        let mut reply = Self::response(401, "text/plain", b"Authentication required".to_vec());
        reply.headers.push(("WWW-Authenticate".into(), format!(
            "Digest realm=\"{REALM}\", nonce=\"{nonce}\", opaque=\"{OPAQUE}\", algorithm={}, qop=\"auth\"{}",
            algorithm.name(), if stale { ", stale=true" } else { "" }
        )));
        reply
    }
    fn redirect(url: &str) -> Self {
        let mut reply = Self::response(302, "text/plain", Vec::new());
        reply.headers.push(("Location".into(), url.into()));
        reply
    }
}
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let handler = Arc::new(handler);
        let worker = thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        assert!(connections.len() < 256, "fixture connection limit");
                        let handler = handler.clone();
                        let seen = seen.clone();
                        connections.push(thread::spawn(move || {
                            if let Some(request) = read_request(&mut stream) {
                                let reply = if request.path == "/favicon.ico" {
                                    Reply::response(204, "text/plain", Vec::new())
                                } else {
                                    handler(&request)
                                };
                                seen.lock().unwrap().push(Recorded { request, status: reply.status });
                                let mut header = format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Length: {}\r\n", reply.status, reply.body.len());
                                for (name, value) in reply.headers {
                                    write!(&mut header, "{name}: {value}\r\n").unwrap();
                                }
                                header.push_str("\r\n");
                                let _ = stream.write_all(header.as_bytes());
                                let _ = stream.write_all(&reply.body);
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
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
    fn recorded(&self, path: &str) -> Vec<Recorded> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.request.path == path)
            .cloned()
            .collect()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
fn read_request(stream: &mut TcpStream) -> Option<Request> {
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
    while !bytes.windows(4).any(|value| value == b"\r\n\r\n") {
        let count = request_reader.read(&mut buffer).ok()?;
        if count == 0 || bytes.len() + count > 32 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut line = text.lines().next()?.split_whitespace();
    Some(Request {
        method: line.next()?.into(),
        path: line.next()?.into(),
        authorization: text.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        }),
    })
}
fn config() -> Value {
    json!({"fetch":{"playwright":{"timeout":8000,"wait_for":"load","extra_wait_ms":0,"skip_auto_scroll":true,"http_credentials":{"username":USER,"password":PASSWORD}}},"screenshot":{"viewport_width":320,"viewport_height":240,"tile_height":0}})
}
fn isolated(name: &str) -> bool {
    if std::env::var("MARKITAI_DIGEST_CASE").as_deref() == Ok(name) {
        return false;
    }
    let directory = tempfile::tempdir().unwrap();
    let stdout = directory.path().join("stdout");
    let stderr = directory.path().join("stderr");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            &format!("browser::digest_tests::{name}"),
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("MARKITAI_DIGEST_CASE", name)
        .env("MARKITAI_HOME", directory.path().join("private-state"))
        .current_dir(directory.path())
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
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("digest fixture child timed out: {name}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = std::fs::read_to_string(stdout).unwrap();
    assert!(
        status.success(),
        "{name}: {output}\n{}",
        std::fs::read_to_string(stderr).unwrap()
    );
    assert!(output.contains("1 passed"));
    true
}

fn fields() -> BTreeMap<String, String> {
    [
        ("username", USER),
        ("realm", REALM),
        ("nonce", FIRST),
        ("opaque", OPAQUE),
        ("uri", "/document?part=2"),
        ("qop", "auth"),
        ("nc", "00000001"),
        ("cnonce", "independent-fixture-cnonce"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}
fn authorization(fields: &BTreeMap<String, String>) -> String {
    format!(
        "Digest {}",
        fields
            .iter()
            .map(|(key, value)| format!(
                "{key}=\"{}\"",
                value.replace('\\', "\\\\").replace('"', "\\\"")
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[test]
fn fixture_verifies_the_full_digest_and_rejects_bad_parameters() {
    for algorithm in [
        Algorithm::Md5,
        Algorithm::Md5Sess,
        Algorithm::Sha256,
        Algorithm::Sha256Sess,
    ] {
        let mut fields = fields();
        fields.insert("algorithm".into(), algorithm.name().into());
        fields.insert(
            "response".into(),
            algorithm.response(&fields, "GET", PASSWORD),
        );
        let request = Request {
            method: "GET".into(),
            path: fields["uri"].clone(),
            authorization: Some(authorization(&fields)),
        };
        assert!(algorithm.verify(&request, FIRST, USER, PASSWORD));
        for (key, value) in [
            ("username", "other"),
            ("realm", "other"),
            ("nonce", SECOND),
            ("opaque", "other"),
            ("uri", "/different"),
            ("qop", "auth-int"),
            ("nc", "00000000"),
            ("nc", "1"),
            ("cnonce", ""),
            ("algorithm", "unknown"),
            ("response", "0000"),
        ] {
            let mut altered = fields.clone();
            altered.insert(key.into(), value.into());
            let mut request = request.clone();
            request.authorization = Some(authorization(&altered));
            assert!(
                !algorithm.verify(&request, FIRST, USER, PASSWORD),
                "{key} must be verified"
            );
        }
        assert!(!algorithm.verify(&request, FIRST, USER, "wrong-password"));
        let mut post = request.clone();
        post.method = "POST".into();
        assert!(!algorithm.verify(&post, FIRST, USER, PASSWORD));
    }
    for invalid in [
        "Basic abc",
        "Digest realm=\"unterminated",
        "Digest a=1,a=2",
        "Digest a=1,A=2",
        "Digest a=1,",
        "Digest a=\"x\" suffix",
        "Digest a=\"line\nvalue\"",
    ] {
        assert!(parameters(invalid).is_none(), "{invalid}");
    }
}

#[test]
fn fixture_digest_matches_independent_known_answers() {
    let rfc: BTreeMap<String, String> = [
        ("username", "Mufasa"),
        ("realm", "testrealm@host.com"),
        ("nonce", "dcd98b7102dd2f0e8b11d0f600bfb0c093"),
        ("uri", "/dir/index.html"),
        ("nc", "00000001"),
        ("cnonce", "0a4f113b"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    assert_eq!(
        Algorithm::Md5.response(&rfc, "GET", "Circle Of Life"),
        "6629fae49393a05397450978507c4ef1"
    );
    // Independently generated using Python hashlib, with the same original ASCII inputs.
    for (algorithm, expected) in [
        (Algorithm::Md5Sess, "8e3825c57e897f5a0dec6c2d4e5059d0"),
        (
            Algorithm::Sha256,
            "5abdd07184ba512a22c53f41470e5eea7dcaa3a93a59b630c13dfe0a5dc6e38b",
        ),
        (
            Algorithm::Sha256Sess,
            "b8822e12417cb7750f4e2b8515f0dcf25b7dd26993e80bee1426201446a7f59b",
        ),
    ] {
        assert_eq!(algorithm.response(&rfc, "GET", "Circle Of Life"), expected);
    }
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_algorithms_render_authenticated_body_and_pixels() {
    if isolated("installed_digest_algorithms_render_authenticated_body_and_pixels") {
        return;
    }
    assert!(available());
    for algorithm in [
        Algorithm::Md5,
        Algorithm::Md5Sess,
        Algorithm::Sha256,
        Algorithm::Sha256Sess,
    ] {
        let server = Server::new(move |request| {
            if algorithm.verify(request, FIRST, USER, PASSWORD) {
                Reply::ok()
            } else {
                Reply::challenge(algorithm, FIRST, false)
            }
        });
        let page = fetch(&server.url("/document?part=2"), &config(), true)
            .unwrap()
            .page();
        assert!(page.html.contains("AUTHENTICATED DIGEST BODY"));
        assert_eq!(page.title, "Digest fixture");
        assert_eq!(page.final_url, server.url("/document?part=2"));
        assert_eq!(page.screenshots.len(), 1);
        let image = image::load_from_memory(&page.screenshots[0].bytes)
            .unwrap()
            .to_rgb8();
        assert_eq!((image.width(), image.height()), (320, 240));
        let pixel = image.get_pixel(5, 200).0;
        assert!(
            (30..=50).contains(&pixel[0])
                && (80..=100).contains(&pixel[1])
                && (150..=170).contains(&pixel[2])
        );
        let requests = server.recorded("/document?part=2");
        assert_eq!(requests.len(), 2, "{} handshake", algorithm.name());
        assert!(requests[0].request.authorization.is_none());
        assert_eq!(
            requests.iter().filter(|entry| entry.status == 200).count(),
            1
        );
        assert!(algorithm.verify(&requests[1].request, FIRST, USER, PASSWORD));
    }
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_stale_nonce_is_renewed_inside_chromium() {
    if isolated("installed_digest_stale_nonce_is_renewed_inside_chromium") {
        return;
    }
    assert!(available());
    for algorithm in [Algorithm::Md5, Algorithm::Sha256] {
        let accepted = Arc::new(AtomicUsize::new(0));
        let old_nonce = accepted.clone();
        let server = Server::new(move |request| {
            if algorithm.verify(request, FIRST, USER, PASSWORD) {
                old_nonce.fetch_add(1, Ordering::Relaxed);
                Reply::challenge(algorithm, SECOND, true)
            } else if algorithm.verify(request, SECOND, USER, PASSWORD) {
                Reply::ok()
            } else {
                Reply::challenge(algorithm, FIRST, false)
            }
        });
        let page = fetch(&server.url("/stale"), &config(), false)
            .unwrap()
            .page();
        assert!(page.html.contains("AUTHENTICATED DIGEST BODY"));
        let requests = server.recorded("/stale");
        assert_eq!(requests.len(), 3);
        assert_eq!(accepted.load(Ordering::Relaxed), 1);
        assert!(requests[0].request.authorization.is_none());
        assert!(algorithm.verify(&requests[1].request, FIRST, USER, PASSWORD));
        assert!(algorithm.verify(&requests[2].request, SECOND, USER, PASSWORD));
        assert_eq!(
            requests.iter().filter(|entry| entry.status == 200).count(),
            1
        );
    }
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_wrong_secret_discards_runtime_without_replaying_navigation() {
    if isolated("installed_digest_wrong_secret_discards_runtime_without_replaying_navigation") {
        return;
    }
    assert!(available());
    let algorithm = Algorithm::Sha256;
    let server = Server::new(move |request| {
        if algorithm.verify(request, FIRST, USER, PASSWORD) {
            Reply::ok()
        } else {
            Reply::challenge(algorithm, FIRST, false)
        }
    });
    let runtime = BrowserRuntime::new(1).unwrap();
    let mut wrong = config();
    wrong["fetch"]["playwright"]["session_mode"] = json!("domain_persistent");
    wrong["fetch"]["playwright"]["http_credentials"]["password"] = json!("wrong-fixture-secret");
    let error = fetch_with_runtime(&server.url("/denied"), &wrong, false, Some(&runtime))
        .err()
        .unwrap()
        .to_string();
    assert!(
        !error.contains(USER)
            && !error.contains(PASSWORD)
            && !error.contains("wrong-fixture-secret")
    );
    assert!(runtime.pool.processes().is_empty());
    let requests = server.recorded("/denied");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].request.authorization.is_none());
    assert!(algorithm.verify(&requests[1].request, FIRST, USER, "wrong-fixture-secret"));
    assert!(requests.iter().all(|entry| entry.status == 401));
    let page = fetch_with_runtime(&server.url("/recovered"), &config(), false, Some(&runtime))
        .unwrap()
        .page();
    assert!(page.html.contains("AUTHENTICATED DIGEST BODY"));
    assert_eq!(server.recorded("/recovered").len(), 2);
    runtime.close();
    assert!(runtime.pool.processes().is_empty());
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_redirect_requires_the_explicit_target_origin() {
    if isolated("installed_digest_redirect_requires_the_explicit_target_origin") {
        return;
    }
    assert!(available());
    let algorithm = Algorithm::Sha256;
    let target = Server::new(move |request| {
        if algorithm.verify(request, FIRST, USER, PASSWORD) {
            Reply::ok()
        } else {
            Reply::challenge(algorithm, FIRST, false)
        }
    });
    let destination = target.url("/protected");
    let source = Server::new(move |_| Reply::redirect(&destination));
    assert!(fetch(&source.url("/start"), &config(), false).is_err());
    let denied = target.recorded("/protected");
    assert_eq!(denied.len(), 1);
    assert!(
        denied
            .iter()
            .all(|entry| entry.request.authorization.is_none())
    );
    let mut explicit = config();
    explicit["fetch"]["playwright"]["http_credentials"]["origin"] = json!(target.origin);
    let page = fetch(&source.url("/start"), &explicit, false)
        .unwrap()
        .page();
    assert!(page.html.contains("AUTHENTICATED DIGEST BODY"));
    assert_eq!(page.final_url, target.url("/protected"));
    assert!(
        source
            .recorded("/start")
            .iter()
            .all(|entry| entry.request.authorization.is_none())
    );
    let requests = target.recorded("/protected");
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests.iter().filter(|entry| entry.status == 200).count(),
        1
    );
    assert!(algorithm.verify(&requests[2].request, FIRST, USER, PASSWORD));
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_third_party_challenge_never_receives_the_server_identity() {
    if isolated("installed_digest_third_party_challenge_never_receives_the_server_identity") {
        return;
    }
    assert!(available());
    let third_party = Server::new(|_| Reply::challenge(Algorithm::Sha256, FIRST, false));
    let frame = third_party.url("/frame");
    let server = Server::new(move |request| {
        if !Algorithm::Sha256.verify(request, FIRST, USER, PASSWORD) {
            return Reply::challenge(Algorithm::Sha256, FIRST, false);
        }
        Reply::response(
            200,
            "text/html",
            format!("<h1>AUTHENTICATED DIGEST BODY</h1><iframe src='{frame}'></iframe>")
                .into_bytes(),
        )
    });
    let page = fetch(&server.url("/document"), &config(), false)
        .unwrap()
        .page();
    assert!(page.html.contains("AUTHENTICATED DIGEST BODY"));
    let requests = third_party.recorded("/frame");
    assert_eq!(requests.len(), 1);
    assert!(
        requests
            .iter()
            .all(|entry| entry.request.authorization.is_none())
    );
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_redirected_extensionless_pdf_uses_one_authenticated_body() {
    if isolated("installed_digest_redirected_extensionless_pdf_uses_one_authenticated_body") {
        return;
    }
    assert!(available());
    let algorithm = Algorithm::Sha256;
    let server = Server::new(move |request| {
        if request.path == "/start" {
            return Reply::redirect("/download?part=2");
        }
        if algorithm.verify(request, FIRST, USER, PASSWORD) {
            Reply::response(200, "application/pdf", PDF.to_vec())
        } else {
            Reply::challenge(algorithm, FIRST, false)
        }
    });
    let response = fetch(&server.url("/start"), &config(), true).unwrap();
    let BrowserResponse::Pdf(pdf) = response else {
        panic!("PDF must not become viewer HTML");
    };
    assert_eq!(pdf.bytes, PDF);
    assert_eq!(pdf.final_url, server.url("/download?part=2"));
    let requests = server.recorded("/download?part=2");
    assert_eq!(server.recorded("/start").len(), 1);
    assert_eq!(requests.len(), 2);
    assert!(requests[0].request.authorization.is_none());
    assert!(algorithm.verify(&requests[1].request, FIRST, USER, PASSWORD));
    assert_eq!(
        requests.iter().filter(|entry| entry.status == 200).count(),
        1
    );
}

#[test]
#[ignore = "requires installed Chromium; private loopback Digest verification only"]
fn installed_digest_persistent_accounts_have_distinct_process_identity() {
    if isolated("installed_digest_persistent_accounts_have_distinct_process_identity") {
        return;
    }
    assert!(available());
    let server = Server::new(move |request| {
        for (user, password) in [
            (USER, PASSWORD),
            ("second-fixture-user", "second-fixture-password"),
        ] {
            if Algorithm::Sha256.verify(request, FIRST, user, password) {
                return Reply::response(
                    200,
                    "text/html",
                    format!("<h1>ACCOUNT {user}</h1>").into_bytes(),
                );
            }
        }
        Reply::challenge(Algorithm::Sha256, FIRST, false)
    });
    let runtime = BrowserRuntime::new(2).unwrap();
    let mut cfg = config();
    cfg["fetch"]["playwright"]["session_mode"] = json!("domain_persistent");
    let first = fetch_with_runtime(&server.url("/first"), &cfg, false, Some(&runtime))
        .unwrap()
        .page();
    assert!(first.html.contains(&format!("ACCOUNT {USER}")));
    let first_processes = runtime.pool.processes();
    assert_eq!(first_processes.len(), 1);
    let mut other = cfg.clone();
    other["fetch"]["playwright"]["http_credentials"] =
        json!({"username":"second-fixture-user","password":"second-fixture-password"});
    let second = fetch_with_runtime(&server.url("/second"), &other, false, Some(&runtime))
        .unwrap()
        .page();
    assert!(second.html.contains("ACCOUNT second-fixture-user"));
    let processes = runtime.pool.processes();
    assert_eq!(processes.len(), 2);
    assert!(processes.contains(&first_processes[0]));
    let again = fetch_with_runtime(&server.url("/first-again"), &cfg, false, Some(&runtime))
        .unwrap()
        .page();
    assert!(again.html.contains(&format!("ACCOUNT {USER}")));
    assert_eq!(runtime.pool.processes(), processes);
    runtime.close();
    assert!(runtime.pool.processes().is_empty());
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
