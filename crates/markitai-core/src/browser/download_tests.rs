//! Installed-browser tests use only original PDF bytes and private loopback servers.
use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

type Handler = dyn Fn(&Request) -> Reply + Send + Sync;
#[derive(Clone)]
struct Request {
    path: String,
    auth: Option<String>,
}
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    bytes: Vec<u8>,
    delay: Duration,
}
impl Reply {
    fn response(status: u16, mime: &str, bytes: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), mime.into())],
            bytes,
            delay: Duration::ZERO,
        }
    }
    fn denied() -> Self {
        let mut reply = Self::response(401, "text/plain", b"Auth required".to_vec());
        reply
            .headers
            .push(("WWW-Authenticate".into(), "Basic realm=\"download\"".into()));
        reply
    }
    fn redirect(url: String) -> Self {
        let mut reply = Self::response(302, "text/plain", Vec::new());
        reply.headers.push(("Location".into(), url));
        reply
    }
}
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handler: Arc<Handler> = Arc::new(handler);
        let worker =
            thread::spawn(move || {
                let mut connections = Vec::new();
                while !stopped.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let handler = handler.clone();
                            let seen = seen.clone();
                            connections.push(thread::spawn(move || {
                                if let Some(request) = read_request(&stream) {
                                    let reply = handler(&request);
                                    seen.lock().unwrap().push(request);
                                    let mut stream = stream;
                                    let mut head = format!(
                                        "HTTP/1.1 {} Fixture\r\nConnection: close\r\n",
                                        reply.status
                                    );
                                    if !reply.headers.iter().any(|(name, _)| {
                                        name.eq_ignore_ascii_case("content-length")
                                    }) {
                                        head.push_str(&format!(
                                            "Content-Length: {}\r\n",
                                            reply.bytes.len()
                                        ));
                                    }
                                    for (name, value) in reply.headers {
                                        head.push_str(&format!("{name}: {value}\r\n"));
                                    }
                                    head.push_str("\r\n");
                                    let _ = stream.write_all(head.as_bytes());
                                    thread::sleep(reply.delay);
                                    let _ = stream.write_all(&reply.bytes);
                                }
                            }));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    }
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
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
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
    let mut buf = [0; 4096];
    while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
        let n = request_reader.read(&mut buf).ok()?;
        if n == 0 || bytes.len() + n > 32 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    Some(Request {
        path: text.lines().next()?.split_whitespace().nth(1)?.into(),
        auth: text.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().into())
        }),
    })
}
fn credentials() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("pdf-reader:fixture-secret")
    )
}
fn cfg() -> Value {
    json!({"fetch":{"playwright":{"timeout":8000,"wait_for":"load","skip_auto_scroll":true,"extra_wait_ms":0,"http_credentials":{"username":"pdf-reader","password":"fixture-secret"}}}})
}
fn pdf() -> Vec<u8> {
    let content = b"BT /F1 12 Tf 20 90 Td (Original authenticated PDF body) Tj ET";
    let objects=[b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 240 120] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_vec(),b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),[format!("<< /Length {} >>\nstream\n",content.len()).as_bytes(),content,b"\nendstream"].concat()];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    bytes
}
fn downloaded(response: BrowserResponse) -> BrowserPdf {
    match response {
        BrowserResponse::Pdf(pdf) => pdf,
        BrowserResponse::Page(_) => panic!("PDF must not become viewer HTML"),
    }
}

#[test]
#[ignore = "requires installed Chromium; authored loopback downloads only"]
fn installed_chromium_pdf_auth_inline_attachment_and_redirect_use_one_body() {
    for attachment in [false, true] {
        let bytes = pdf();
        let served = bytes.clone();
        let server = Server::new(move |request| {
            if request.auth.as_deref() != Some(credentials().as_str()) {
                return Reply::denied();
            }
            let mut reply = Reply::response(
                200,
                if attachment {
                    "application/octet-stream"
                } else {
                    "application/pdf"
                },
                served.clone(),
            );
            if attachment {
                reply.headers.push((
                    "Content-Disposition".into(),
                    "attachment; filename=\"report.pdf\"".into(),
                ));
            }
            reply
        });
        let result = downloaded(fetch(&server.url("/extensionless"), &cfg(), true).unwrap());
        assert_eq!(result.bytes, bytes);
        assert_eq!(result.final_url, server.url("/extensionless"));
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].auth.is_none());
        assert_eq!(requests[1].auth.as_deref(), Some(credentials().as_str()));
    }
    let bytes = pdf();
    let served = bytes.clone();
    let server = Server::new(move |request| {
        if request.auth.as_deref() != Some(credentials().as_str()) {
            return Reply::denied();
        }
        if request.path == "/start" {
            return Reply::redirect("/final".into());
        }
        Reply::response(200, "application/pdf", served.clone())
    });
    let result = downloaded(fetch(&server.url("/start"), &cfg(), false).unwrap());
    assert_eq!(result.bytes, bytes);
    assert_eq!(result.final_url, server.url("/final"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/final" && r.auth.as_deref() == Some(credentials().as_str()))
            .count(),
        1
    );
}

#[test]
#[ignore = "requires installed Chromium; authored loopback downloads only"]
fn installed_chromium_download_auth_scope_and_text_priority() {
    let denied = Server::new(|_| Reply::denied());
    assert!(fetch(&denied.url("/private"), &cfg(), false).is_err());
    assert_eq!(denied.requests().len(), 2);
    assert_eq!(
        denied
            .requests()
            .iter()
            .filter(|r| r.auth.is_some())
            .count(),
        1
    );
    let bytes = pdf();
    let served = bytes.clone();
    let target = Server::new(move |request| {
        if request.auth.as_deref() == Some(credentials().as_str()) {
            Reply::response(200, "application/pdf", served.clone())
        } else {
            Reply::denied()
        }
    });
    let destination = target.url("/private");
    let redirect = Server::new(move |_| Reply::redirect(destination.clone()));
    assert!(fetch(&redirect.url("/start"), &cfg(), false).is_err());
    assert!(target.requests().iter().all(|r| r.auth.is_none()));
    let mut options = cfg();
    options["fetch"]["playwright"]["http_credentials"]["origin"] = json!(target.base);
    let actual = downloaded(fetch(&redirect.url("/start"), &options, false).unwrap());
    assert_eq!(actual.bytes, bytes);
    let text = Server::new(|_| {
        Reply::response(
            200,
            "text/plain",
            b"%PDF-1.4 literal example, not a PDF document".to_vec(),
        )
    });
    let page = fetch(&text.url("/example.pdf"), &cfg(), false)
        .unwrap()
        .page();
    assert!(page.html.contains("literal example"));
    assert_eq!(
        text.requests()
            .iter()
            .filter(|r| r.path == "/example.pdf")
            .count(),
        1
    );
}

#[test]
#[ignore = "requires installed Chromium; authored loopback downloads only"]
fn installed_chromium_download_limits_and_incomplete_body_fail_closed() {
    let too_large = Server::new(|_| {
        let mut reply = Reply::response(200, "application/pdf", Vec::new());
        reply.headers.push((
            "Content-Length".into(),
            (super::download::LIMIT + 1).to_string(),
        ));
        reply
    });
    let error = fetch(&too_large.url("/huge"), &cfg(), false)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("100 MiB"));
    assert_eq!(too_large.requests().len(), 1);
    let delayed = Server::new(|_| {
        let mut reply = Reply::response(200, "application/pdf", pdf());
        reply.delay = Duration::from_millis(2200);
        reply
    });
    let mut options = cfg();
    options["fetch"]["playwright"]["timeout"] = json!(1000);
    let start = Instant::now();
    assert!(fetch(&delayed.url("/slow"), &options, false).is_err());
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(delayed.requests().len(), 1);
    let truncated = Server::new(|_| {
        let bytes = pdf();
        let mut reply = Reply::response(200, "application/pdf", bytes.clone());
        reply
            .headers
            .push(("Content-Length".into(), (bytes.len() + 100).to_string()));
        reply
    });
    assert!(fetch(&truncated.url("/short"), &cfg(), false).is_err());
    assert_eq!(truncated.requests().len(), 1);
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
