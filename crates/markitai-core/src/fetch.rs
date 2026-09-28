use crate::{Document, Error, Result, config, fetch_cache, formats, output};
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;
use url::Url;

const MAX_RESPONSE: u64 = 100 * 1024 * 1024;

pub(crate) fn client(timeout: u64) -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(timeout))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("markitai/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Fetch(e.without_url().to_string()))
}

pub(crate) fn body(response: Response) -> Result<Vec<u8>> {
    if !response.status().is_success() {
        return Err(Error::Fetch(format!("HTTP {}", response.status().as_u16())));
    }
    if response.content_length().is_some_and(|n| n > MAX_RESPONSE) {
        return Err(Error::Fetch("Response exceeds 100 MiB".into()));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Fetch(format!("Cannot read HTTP response: {e}")))?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Error::Fetch("Response exceeds 100 MiB".into()));
    }
    Ok(bytes)
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_multicast()
                || v.octets()[0] == 0
                || v.octets()[0] >= 240
                || (v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        IpAddr::V6(v) => {
            v.is_loopback()
                || v.is_unspecified()
                || v.is_unique_local()
                || v.is_unicast_link_local()
                || v.is_multicast()
                || v.to_ipv4_mapped()
                    .is_some_and(|v| is_private(IpAddr::V4(v)))
        }
    }
}

fn remote_allowed(url: &Url, cfg: &Value) -> Result<()> {
    let env = config::environment();
    if env
        .get("MARKITAI_NO_REMOTE_FETCH")
        .is_some_and(|s| ["1", "true", "yes", "on"].contains(&s.to_lowercase().as_str()))
        || cfg.pointer("/fetch/remote_consent").and_then(Value::as_str) != Some("always")
    {
        return Err(Error::Fetch("Remote fetching is disabled by policy".into()));
    }
    let sensitive_query = url.query_pairs().any(|(key, _)| {
        [
            "token",
            "key",
            "secret",
            "password",
            "signature",
            "credential",
        ]
        .iter()
        .any(|part| key.to_lowercase().contains(part))
    });
    if !url.username().is_empty() || url.password().is_some() || sensitive_query {
        return Err(Error::Fetch(
            "Credentialed URLs cannot be sent to remote extraction services".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::Fetch("URL has no hostname".into()))?;
    if !host.contains('.')
        || host.ends_with(".local")
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
    {
        return Err(Error::Fetch(
            "Local URLs cannot be sent to remote extraction services".into(),
        ));
    }
    let addresses: Vec<_> = (host, url.port_or_known_default().unwrap_or(443))
        .to_socket_addrs()
        .map_err(|_| Error::Fetch("Cannot resolve URL hostname for remote policy check".into()))?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|a| is_private(a.ip())) {
        return Err(Error::Fetch(
            "Private URLs cannot be sent to remote extraction services".into(),
        ));
    }
    Ok(())
}

pub(crate) struct FetchOutcome {
    pub document: Document,
    pub cache_hit: bool,
}

#[cfg(test)]
pub fn fetch(source: &str, cfg: &Value) -> Result<Document> {
    Ok(fetch_with_context(source, cfg, None)?.document)
}

/// Explicit strategy provenance affects the cache key, not strategy selection.
pub(crate) fn fetch_with_context(
    source: &str,
    cfg: &Value,
    explicit_strategy: Option<&str>,
) -> Result<FetchOutcome> {
    let url = Url::parse(source).map_err(|_| Error::InvalidInput("Invalid URL".into()))?;
    if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
        return Err(Error::InvalidInput(
            "An HTTP(S) URL with a hostname is required".into(),
        ));
    }
    let strategy = cfg
        .pointer("/fetch/strategy")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    match strategy {
        "auto" | "static" => fetch_static(source, &url, cfg, explicit_strategy),
        "defuddle" | "jina" => {
            remote_allowed(&url, cfg)?;
            let client = client(30)?;
            let remote = if strategy == "defuddle" {
                format!(
                    "https://defuddle.md/{}",
                    url::form_urlencoded::byte_serialize(source.as_bytes()).collect::<String>()
                )
            } else {
                format!("https://r.jina.ai/{source}")
            };
            let mut request = client.get(remote);
            if strategy == "jina" {
                request = request.header("Accept", "application/json");
                if let Some(key) = config::environment().get("JINA_API_KEY") {
                    request = request.bearer_auth(key);
                }
            }
            let bytes = body(
                request
                    .send()
                    .map_err(|e| Error::Fetch(e.without_url().to_string()))?,
            )?;
            let mut doc = if strategy == "jina" {
                let value: Value = serde_json::from_slice(&bytes)?;
                let markdown = value
                    .pointer("/data/content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Fetch("Jina returned no content".into()))?
                    .to_owned();
                let mut doc = Document {
                    markdown,
                    ..Default::default()
                };
                if let Some(title) = value.pointer("/data/title") {
                    doc.metadata.insert("title".into(), title.clone());
                }
                doc
            } else {
                let text = String::from_utf8(bytes)
                    .map_err(|_| Error::Fetch("Remote Markdown is not UTF-8".into()))?;
                let (metadata, markdown) = output::split_frontmatter(&text);
                Document {
                    markdown: markdown.into(),
                    metadata,
                    ..Default::default()
                }
            };
            if doc.markdown.trim().is_empty() {
                return Err(Error::Fetch("Remote service returned empty content".into()));
            }
            doc.metadata
                .insert("fetch_strategy".into(), json!(strategy));
            Ok(FetchOutcome {
                document: doc,
                cache_hit: false,
            })
        }
        _ => Err(Error::Unsupported(format!(
            "Fetch strategy '{strategy}' is not implemented in this development build"
        ))),
    }
}

const STATIC_ACCEPT: &str = "text/markdown, text/html;q=0.9, */*;q=0.5";
const CACHE_WARNING: &str =
    "Persistent URL fetch cache is unavailable; caching could not be completed.";

struct StaticPage {
    document: Document,
    cache_eligible: bool,
    final_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

fn cache_warning(document: &mut Document, unavailable: bool) {
    if unavailable {
        document.warnings.push(CACHE_WARNING.into());
    }
}

fn cached_document(entry: fetch_cache::Entry) -> Document {
    let mut metadata = entry.metadata;
    if let Some(title) = entry.title {
        metadata.entry("title").or_insert_with(|| title.into());
    }
    metadata.insert("fetch_strategy".into(), entry.strategy_used.into());
    Document {
        markdown: entry.content,
        metadata,
        ..Default::default()
    }
}

fn fetch_static(
    source: &str,
    url: &Url,
    cfg: &Value,
    explicit_strategy: Option<&str>,
) -> Result<FetchOutcome> {
    let cache = fetch_cache::Cache::from_config(cfg);
    let mut cache_unavailable = false;
    let entry = match cache
        .as_ref()
        .map(|cache| cache.get(source, explicit_strategy))
    {
        Some(Ok(entry)) => entry.filter(|entry| !entry.content.trim().is_empty()),
        Some(Err(_)) => {
            cache_unavailable = true;
            None
        }
        None => None,
    };
    let entry = match entry {
        Some(entry)
            if entry.etag.as_deref().is_none_or(str::is_empty)
                && entry.last_modified.as_deref().is_none_or(str::is_empty) =>
        {
            // The store enforces TTL and updates access time on reads.
            let mut document = cached_document(entry);
            cache_warning(&mut document, cache_unavailable);
            return Ok(FetchOutcome {
                document,
                cache_hit: true,
            });
        }
        entry => entry,
    };

    let client = client(30)?;
    let mut fresh = None;
    if let Some(entry) = entry {
        let mut request = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, STATIC_ACCEPT);
        if let Some(etag) = &entry.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &entry.last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
        }
        if let Ok(response) = request.send() {
            if response.status() == reqwest::StatusCode::NOT_MODIFIED {
                if let Some(cache) = &cache {
                    cache_unavailable |= cache.touch(source, explicit_strategy).is_err();
                }
                let mut document = cached_document(entry);
                cache_warning(&mut document, cache_unavailable);
                return Ok(FetchOutcome {
                    document,
                    cache_hit: true,
                });
            }
            // A failed status, unreadable body or unusable extracted page takes
            // the same unconditional path as a failed conditional request.
            fresh = decode_static(response).ok();
        }
    }
    let mut page = match fresh {
        Some(page) => page,
        None => decode_static(
            client
                .get(url.clone())
                .header(reqwest::header::ACCEPT, STATIC_ACCEPT)
                .send()
                .map_err(|e| Error::Fetch(e.without_url().to_string()))?,
        )?,
    };
    if let Some(cache) = cache {
        if page.cache_eligible
            && page.document.assets.is_empty()
            && page.document.warnings.is_empty()
        {
            let entry = fetch_cache::Entry {
                content: page.document.markdown.clone(),
                metadata: page.document.metadata.clone(),
                strategy_used: "static".into(),
                title: page
                    .document
                    .metadata
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                final_url: Some(page.final_url),
                etag: page.etag,
                last_modified: page.last_modified,
                ..Default::default()
            };
            cache_unavailable |= cache.set(source, explicit_strategy, &entry).is_err();
        } else {
            // A successfully changed representation must not leave an old
            // validator or still-fresh TTL row able to replay different content.
            cache_unavailable |= cache.remove(source, explicit_strategy).is_err();
        }
    }
    cache_warning(&mut page.document, cache_unavailable);
    Ok(FetchOutcome {
        document: page.document,
        cache_hit: false,
    })
}

fn header_text(response: &Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn decode_text<'a>(bytes: &'a [u8], content_type: &str) -> std::borrow::Cow<'a, str> {
    let charset = content_type.split(';').find_map(|parameter| {
        let (key, value) = parameter.trim().split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then_some(value.trim())
    });
    let encoding = charset
        .and_then(|label| {
            encoding_rs::Encoding::for_label(label.trim_matches(['\"', '\'']).as_bytes())
        })
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0
}

// Restrict checks to HTML and visible non-code text. Vendor names in an article,
// literal code examples or a plain-text document are not challenge evidence.
fn html_rejection(html: &str) -> Option<&'static str> {
    let tree = scraper::Html::parse_document(html);
    let mut text = String::new();
    let mut title = String::new();
    let mut widget = false;
    let mut authored_article = false;
    for node in tree.tree.nodes() {
        if let Some(element) = scraper::ElementRef::wrap(node) {
            let name = element.value().name();
            if name == "article" {
                authored_article |= element.text().any(|value| !value.trim().is_empty());
            }
            if name == "title" {
                title = element.text().collect::<String>().to_lowercase();
            }
            for attr in ["id", "class", "src", "action"] {
                if let Some(value) = element.value().attr(attr) {
                    let value = value.to_ascii_lowercase();
                    widget |= [
                        "cf-browser-verification",
                        "cf-chl-",
                        "cf_chl_",
                        "/cdn-cgi/challenge-platform/",
                        "google.com/recaptcha",
                        "hcaptcha.com",
                        "g-recaptcha",
                        "h-captcha",
                    ]
                    .iter()
                    .any(|marker| value.contains(marker));
                }
            }
        } else if let scraper::Node::Text(value) = node.value() {
            let ignored = node
                .ancestors()
                .filter_map(scraper::ElementRef::wrap)
                .any(|element| {
                    matches!(
                        element.value().name(),
                        "head" | "script" | "style" | "pre" | "code" | "template"
                    )
                });
            if !ignored {
                text.push_str(value);
                text.push(' ');
            }
        }
    }
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let short = text.len() <= 2000;
    let challenge_instruction = [
        "checking your browser before accessing",
        "verify you are human",
        "verify that you are human",
        "智能验证检测中",
        "由极验提供技术支持",
    ]
    .iter()
    .any(|phrase| text.starts_with(phrase));
    let challenge_title = [
        "just a moment",
        "attention required",
        "security verification",
        "verify you are human",
    ]
    .iter()
    .any(|phrase| title.trim_start().starts_with(phrase));
    if short
        && ((!authored_article && challenge_instruction)
            || (widget && (challenge_title || text.is_empty())))
    {
        return Some(
            "HTML challenge page cannot be extracted; browser challenge fallback is not implemented",
        );
    }
    if short
        && !authored_article
        && [
            "please enable javascript",
            "javascript is disabled",
            "javascript is not available",
            "you need to enable javascript",
            "enable javascript to continue",
        ]
        .iter()
        .any(|phrase| text.starts_with(phrase))
    {
        return Some(
            "HTML page requires JavaScript; browser rendering fallback is not implemented",
        );
    }
    None
}

fn decode_static(response: Response) -> Result<StaticPage> {
    let effective_url = response.url().clone();
    let content_type = header_text(&response, reqwest::header::CONTENT_TYPE).unwrap_or_default();
    let etag = header_text(&response, reqwest::header::ETAG);
    let last_modified = header_text(&response, reqwest::header::LAST_MODIFIED);
    let bytes = body(response)?;
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let prefix = bytes
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(32)
        .collect::<Vec<_>>();
    // An explicitly labelled text document can itself contain an HTML example.
    // Sniffing must not turn that literal text into an executable-page kind.
    let explicit_text = mime.starts_with("text/") && !mime.contains("html");
    let html = mime.contains("html")
        || (!explicit_text
            && (prefix.to_ascii_lowercase().starts_with(b"<!doctype html")
                || prefix.to_ascii_lowercase().starts_with(b"<html")));
    let text = mime.starts_with("text/") && !mime.contains("xml");
    let cache_eligible = html || text;
    let mut doc = if html {
        let html = decode_text(&bytes, &content_type);
        if let Some(reason) = html_rejection(&html) {
            return Err(Error::Fetch(reason.into()));
        }
        formats::extract_html(&html, Some(effective_url.as_str()))?
    } else if text {
        Document {
            markdown: decode_text(&bytes, &content_type).into_owned(),
            ..Default::default()
        }
    } else {
        let extension = if mime.contains("pdf") {
            "pdf"
        } else {
            std::path::Path::new(effective_url.path())
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        };
        if !formats::supports_extension(extension) {
            return Err(Error::Unsupported(format!(
                "Unsupported URL content type: {mime}"
            )));
        }
        let mut file = tempfile::Builder::new()
            .prefix("markitai-fetch-")
            .suffix(&format!(".{extension}"))
            .tempfile()?;
        file.write_all(&bytes)?;
        formats::extract(file.path())?
    };
    if doc.markdown.trim().is_empty() {
        return Err(Error::Fetch("URL returned no extractable content".into()));
    }
    doc.metadata
        .insert("fetch_strategy".into(), json!("static"));
    Ok(StaticPage {
        document: doc,
        cache_eligible,
        final_url: effective_url.into(),
        etag,
        last_modified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_and_credentialed_remote_targets_are_rejected() {
        let cfg = config::defaults();
        for target in [
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://[::1]/x",
            "https://user:pass@example.com/x",
            "https://example.com/?api_key=secret",
        ] {
            assert!(remote_allowed(&Url::parse(target).unwrap(), &cfg).is_err());
        }
    }
    #[test]
    fn static_html_runs_against_local_http() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let html = "<html><title>Local title</title><article><h1>Local title</h1><p>Hello native network.</p></article></html>";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",html.len()).unwrap();
        });
        let mut cfg = config::defaults();
        cfg["cache"]["enabled"] = json!(false);
        let doc = fetch(&format!("http://{addr}/page"), &cfg).unwrap();
        server.join().unwrap();
        assert!(doc.markdown.contains("Hello native network."));
        assert_eq!(doc.metadata["fetch_strategy"], "static");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone)]
    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Reply {
        fn text(body: &str) -> Self {
            Self {
                status: 200,
                headers: vec![("Content-Type".into(), "text/plain".into())],
                body: body.as_bytes().to_vec(),
            }
        }
        fn html(body: &str) -> Self {
            Self::text(body).header("Content-Type", "text/html")
        }
        fn header(mut self, name: &str, value: &str) -> Self {
            self.headers
                .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            self.headers.push((name.into(), value.into()));
            self
        }
        fn status(mut self, status: u16) -> Self {
            self.status = status;
            self
        }
    }

    struct Server {
        origin: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let log = Arc::clone(&requests);
            let stopped = Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("loopback server: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let size = stream.read(&mut chunk).unwrap();
                        if size == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..size]);
                        assert!(request.len() <= 64 * 1024);
                    }
                    let index = {
                        let mut log = log.lock().unwrap();
                        let index = log.len();
                        log.push(String::from_utf8(request).unwrap());
                        index
                    };
                    let reply = replies
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| Reply::text("unexpected request").status(500));
                    write!(
                        stream,
                        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    )
                    .unwrap();
                    for (name, value) in reply.headers {
                        write!(stream, "{name}: {value}\r\n").unwrap();
                    }
                    stream.write_all(b"\r\n").unwrap();
                    stream.write_all(&reply.body).unwrap();
                }
            });
            Self {
                origin,
                requests,
                stop,
                thread: Some(thread),
            }
        }
        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.origin)
        }
        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let result = self.thread.take().unwrap().join();
            if !std::thread::panicking() {
                result.expect("loopback server failed");
            }
        }
    }
    fn settings() -> (tempfile::TempDir, Value) {
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = config::defaults();
        cfg["cache"]["global_dir"] = json!(directory.path());
        (directory, cfg)
    }
    fn run(server: &Server, cfg: &Value) -> FetchOutcome {
        fetch_with_context(&server.url("/page"), cfg, None).unwrap()
    }
    fn header(request: &str, name: &str) -> Option<String> {
        request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().into())
    }

    #[test]
    fn fresh_unvalidated_hits_reopen_the_store_and_ttl_does_not_restart() {
        let (directory, mut cfg) = settings();
        let server = Server::new(vec![
            Reply::text("first"),
            Reply::text("second"),
            Reply::text("third"),
        ]);
        assert!(!run(&server, &cfg).cache_hit);
        assert!(run(&server, &cfg).cache_hit);
        assert_eq!(server.requests().len(), 1);
        let db = rusqlite::Connection::open(directory.path().join("fetch_cache.db")).unwrap();
        db.execute("UPDATE fetch_cache SET created_at=1", [])
            .unwrap();
        let second = run(&server, &cfg);
        assert!(!second.cache_hit);
        assert_eq!(second.document.markdown, "second");
        cfg["cache"]["fetch_ttl_seconds"] = json!(0);
        let third = run(&server, &cfg);
        assert!(!third.cache_hit);
        assert_eq!(third.document.markdown, "third");
        assert_eq!(server.requests().len(), 3);
        for request in server.requests() {
            assert_eq!(header(&request, "Accept").as_deref(), Some(STATIC_ACCEPT));
            assert!(header(&request, "If-None-Match").is_none());
        }
    }

    #[test]
    fn validators_are_sent_each_time_independent_of_ttl_and_304_keeps_creation() {
        let (directory, mut cfg) = settings();
        cfg["cache"]["fetch_ttl_seconds"] = json!(0);
        let modified = "Mon, 28 Sep 2026 09:00:00 GMT";
        let server = Server::new(vec![
            Reply::html("<title>First title</title><p>First body.</p>")
                .header("ETag", "\"v1\"")
                .header("Last-Modified", modified),
            Reply::text("").status(304).header("ETag", "\"ignored\""),
            Reply::html("<title>Second title</title><p>Second body.</p>").header("ETag", "\"v2\""),
            Reply::text("").status(304),
        ]);
        let first = run(&server, &cfg);
        let db = rusqlite::Connection::open(directory.path().join("fetch_cache.db")).unwrap();
        db.execute("UPDATE fetch_cache SET created_at=1", [])
            .unwrap();
        let cached = run(&server, &cfg);
        assert!(cached.cache_hit);
        assert_eq!(cached.document.markdown, first.document.markdown);
        assert_eq!(cached.document.metadata, first.document.metadata);
        assert_eq!(
            db.query_row("SELECT created_at FROM fetch_cache", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let fresh = run(&server, &cfg);
        assert!(!fresh.cache_hit);
        assert!(fresh.document.markdown.contains("Second body."));
        assert_eq!(fresh.document.metadata["title"], "Second title");
        assert!(run(&server, &cfg).cache_hit);
        let requests = server.requests();
        assert_eq!(requests.len(), 4);
        for request in &requests[1..3] {
            assert_eq!(header(request, "If-None-Match").as_deref(), Some("\"v1\""));
            assert_eq!(
                header(request, "If-Modified-Since").as_deref(),
                Some(modified)
            );
        }
        assert_eq!(
            header(&requests[3], "If-None-Match").as_deref(),
            Some("\"v2\"")
        );
        assert!(header(&requests[3], "If-Modified-Since").is_none());
    }

    #[test]
    fn last_modified_alone_revalidates_and_failed_conditionals_fetch_normally() {
        let (_directory, cfg) = settings();
        let modified = "Mon, 28 Sep 2026 09:00:00 GMT";
        let server = Server::new(vec![
            Reply::text("old").header("Last-Modified", modified),
            Reply::text("failure").status(503),
            Reply::text("replacement"),
        ]);
        assert!(!run(&server, &cfg).cache_hit);
        let fresh = run(&server, &cfg);
        assert!(!fresh.cache_hit);
        assert_eq!(fresh.document.markdown, "replacement");
        assert!(run(&server, &cfg).cache_hit);
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            header(&requests[1], "If-Modified-Since").as_deref(),
            Some(modified)
        );
        assert!(header(&requests[1], "If-None-Match").is_none());
        assert!(header(&requests[2], "If-Modified-Since").is_none());
    }

    #[test]
    fn redirects_and_declared_charsets_share_the_same_decoder() {
        let (directory, cfg) = settings();
        let mut page = Reply::html("")
            .header("Content-Type", "TEXT/HTML; Charset=\"windows-1252\"")
            .header("ETag", "W/\"page\"");
        page.body =
            b"<title>Caf\xe9</title><article><p>Caf\xe9 <a href=\"child\">link</a></p></article>"
                .to_vec();
        let server = Server::new(vec![
            Reply::text("")
                .status(302)
                .header("Location", "/folder/page"),
            page,
            Reply::text("")
                .status(302)
                .header("Location", "/folder/page"),
            Reply::text("").status(304),
        ]);
        let first = run(&server, &cfg);
        assert!(first.document.markdown.contains("Café"));
        assert!(
            first
                .document
                .markdown
                .contains(&server.url("/folder/child"))
        );
        let second = run(&server, &cfg);
        assert!(second.cache_hit);
        assert_eq!(second.document.markdown, first.document.markdown);
        let db = rusqlite::Connection::open(directory.path().join("fetch_cache.db")).unwrap();
        let final_url: String = db
            .query_row("SELECT final_url FROM fetch_cache", [], |row| row.get(0))
            .unwrap();
        assert_eq!(final_url, server.url("/folder/page"));
        assert_eq!(server.requests().len(), 4);
    }

    #[test]
    fn bypass_refreshes_then_reuses_and_patterns_apply_to_real_requests() {
        let (_directory, mut cfg) = settings();
        let server = Server::new(vec![
            Reply::text("original").header("ETag", "v1"),
            Reply::text("refresh"),
            Reply::text("pattern refresh"),
        ]);
        run(&server, &cfg);
        cfg["cache"]["no_cache"] = json!(true);
        assert_eq!(run(&server, &cfg).document.markdown, "refresh");
        assert!(header(&server.requests()[1], "If-None-Match").is_none());
        cfg["cache"]["no_cache"] = json!(false);
        assert!(run(&server, &cfg).cache_hit);
        cfg["cache"]["no_cache_patterns"] = json!(["**/page"]);
        assert_eq!(run(&server, &cfg).document.markdown, "pattern refresh");
        cfg["cache"]["no_cache_patterns"] = json!([]);
        assert!(run(&server, &cfg).cache_hit);
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn strategy_provenance_uses_distinct_keys_without_bypassing_guards() {
        let (_directory, mut cfg) = settings();
        let server = Server::new(vec![
            Reply::text("unscoped"),
            Reply::text("explicit static"),
        ]);
        assert!(!run(&server, &cfg).cache_hit);
        cfg["fetch"]["strategy"] = json!("static");
        assert!(run(&server, &cfg).cache_hit);
        let explicit = fetch_with_context(&server.url("/page"), &cfg, Some("static")).unwrap();
        assert!(!explicit.cache_hit);
        assert_eq!(explicit.document.markdown, "explicit static");
        let auto = fetch_with_context(&server.url("/page"), &cfg, Some("auto")).unwrap();
        assert!(auto.cache_hit);
        assert_eq!(auto.document.markdown, "unscoped");
        cfg["fetch"]["strategy"] = json!("playwright");
        assert!(matches!(
            fetch_with_context(&server.url("/page"), &cfg, None),
            Err(Error::Unsupported(_))
        ));
        cfg["fetch"]["strategy"] = json!("jina");
        assert!(fetch_with_context(&server.url("/page"), &cfg, None).is_err());
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn disabled_and_unwritable_storage_never_damage_successful_fetches() {
        let (directory, mut cfg) = settings();
        let blocked = directory.path().join("private-token-path");
        std::fs::write(&blocked, "file instead of directory").unwrap();
        cfg["cache"]["global_dir"] = json!(blocked);
        cfg["cache"]["enabled"] = json!(false);
        let server = Server::new(vec![
            Reply::text("first"),
            Reply::text("second"),
            Reply::text("third"),
        ]);
        let first = run(&server, &cfg);
        assert!(!first.cache_hit);
        assert!(first.document.warnings.is_empty());
        assert_eq!(run(&server, &cfg).document.markdown, "second");
        cfg["cache"]["enabled"] = json!(true);
        let third = run(&server, &cfg);
        assert_eq!(third.document.markdown, "third");
        assert_eq!(third.document.warnings, [CACHE_WARNING]);
        assert!(!third.document.warnings[0].contains("private-token-path"));
        assert!(!directory.path().join("fetch_cache.db").exists());
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn empty_error_and_unsolicited_304_responses_do_not_create_cache() {
        for reply in [
            Reply::text("  \n"),
            Reply::text("failure").status(500),
            Reply::text("").status(304),
        ] {
            let (directory, cfg) = settings();
            let server = Server::new(vec![reply]);
            assert!(fetch_with_context(&server.url("/page"), &cfg, None).is_err());
            assert!(!directory.path().join("fetch_cache.db").exists());
            assert_eq!(server.requests().len(), 1);
        }
    }

    #[test]
    fn unusable_revalidation_retains_good_entry_without_silent_stale_success() {
        for invalid in [
            Reply::html(
                "<title>Just a moment...</title><script src=\"/cdn-cgi/challenge-platform/check\"></script><p>Checking your browser before accessing this site.</p>",
            ),
            Reply::html("<p>Please enable JavaScript to continue.</p>"),
            Reply::text(" \n"),
            Reply::text("failure").status(503),
        ] {
            let (_directory, cfg) = settings();
            let server = Server::new(vec![
                Reply::text("saved").header("ETag", "v1"),
                invalid.clone(),
                invalid,
                Reply::text("").status(304),
            ]);
            run(&server, &cfg);
            assert!(fetch_with_context(&server.url("/page"), &cfg, None).is_err());
            let saved = run(&server, &cfg);
            assert!(saved.cache_hit);
            assert_eq!(saved.document.markdown, "saved");
            let requests = server.requests();
            assert_eq!(requests.len(), 4);
            assert!(header(&requests[2], "If-None-Match").is_none());
            assert_eq!(header(&requests[3], "If-None-Match").as_deref(), Some("v1"));
        }
    }

    #[test]
    fn challenge_signatures_in_documents_and_code_are_not_rejected() {
        for reply in [
            Reply::text("Checking your browser before accessing: g-recaptcha cf_chl_ hcaptcha.com"),
            Reply::html(
                "<title>CAPTCHA guide</title><article><p>Explain CAPTCHA integrations.</p><pre><code>Checking your browser before accessing: cf_chl_</code></pre></article>",
            ),
            Reply::html(
                "<title>Contact us</title><p>Send your message.</p><div class=\"g-recaptcha\"></div>",
            ),
            Reply::html(
                "<article><h1>Verify you are human</h1><p>This tutorial explains how identity challenges work.</p></article>",
            ),
            Reply::html(
                "<article><h1>Please enable JavaScript</h1><p>This guide explains the browser setting and progressive enhancement.</p></article>",
            ),
        ] {
            let (_directory, cfg) = settings();
            let server = Server::new(vec![reply]);
            assert!(!run(&server, &cfg).cache_hit);
            assert!(run(&server, &cfg).cache_hit);
            assert_eq!(server.requests().len(), 1);
        }
    }

    #[test]
    fn explicit_text_types_preserve_literal_html_challenge_examples() {
        for content_type in ["text/plain", "text/markdown"] {
            let (_directory, cfg) = settings();
            let example = "<html><body><p>Checking your browser before accessing this site.</p></body></html>";
            let server = Server::new(vec![
                Reply::text(example).header("Content-Type", content_type),
            ]);
            let fresh = run(&server, &cfg);
            assert_eq!(fresh.document.markdown, example);
            assert!(!fresh.cache_hit);
            let cached = run(&server, &cfg);
            assert_eq!(cached.document.markdown, example);
            assert!(cached.cache_hit);
            assert_eq!(server.requests().len(), 1);
        }
    }

    #[test]
    fn downloaded_documents_with_owned_assets_bypass_the_page_cache() {
        let (directory, cfg) = settings();
        let reply = downloadable_email();
        let server = Server::new(vec![reply.clone(), reply]);
        for _ in 0..2 {
            let outcome = fetch_with_context(&server.url("/document.eml"), &cfg, None).unwrap();
            assert!(!outcome.cache_hit);
            assert_eq!(outcome.document.assets.len(), 1);
            assert_eq!(outcome.document.assets[0].bytes, [0, 1, 2, 255]);
            assert!(
                outcome
                    .document
                    .markdown
                    .contains(&outcome.document.assets[0].name)
            );
        }
        assert_eq!(server.requests().len(), 2);
        assert!(header(&server.requests()[1], "If-None-Match").is_none());
        assert!(!directory.path().join("fetch_cache.db").exists());
    }

    fn downloadable_email() -> Reply {
        let mail = "Subject: Download\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nDocument body.\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=data.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC/w==\r\n--x--\r\n";
        Reply::text(mail)
            .header("Content-Type", "application/octet-stream")
            .header("ETag", "mail-v1")
    }

    #[test]
    fn successful_binary_transition_invalidates_previous_page_and_validator() {
        for validators in [true, false] {
            let (directory, mut cfg) = settings();
            let mut initial = Reply::text("old page without attachments");
            if validators {
                initial = initial.header("ETag", "old-page");
            }
            let reply = downloadable_email();
            let server = Server::new(vec![initial, reply.clone(), reply]);
            let url = server.url("/document.eml");
            assert!(!fetch_with_context(&url, &cfg, None).unwrap().cache_hit);
            // Without validators, force a refresh while the old TTL is fresh.
            cfg["cache"]["no_cache"] = json!(!validators);
            let fresh = fetch_with_context(&url, &cfg, None).unwrap();
            assert!(!fresh.cache_hit);
            assert_eq!(fresh.document.assets[0].bytes, [0, 1, 2, 255]);
            cfg["cache"]["no_cache"] = json!(false);
            let next = fetch_with_context(&url, &cfg, None).unwrap();
            assert!(!next.cache_hit);
            assert_eq!(next.document.assets[0].bytes, [0, 1, 2, 255]);
            let requests = server.requests();
            assert_eq!(requests.len(), 3);
            assert!(header(&requests[2], "If-None-Match").is_none());
            let db = rusqlite::Connection::open(directory.path().join("fetch_cache.db")).unwrap();
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM fetch_cache", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
}
