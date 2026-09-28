use crate::{Document, Error, Result, config, formats, output};
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

pub fn fetch(source: &str, cfg: &Value) -> Result<Document> {
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
        "auto" | "static" => fetch_static(&url, cfg),
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
            Ok(doc)
        }
        _ => Err(Error::Unsupported(format!(
            "Fetch strategy '{strategy}' is not implemented in this development build"
        ))),
    }
}

fn fetch_static(url: &Url, _cfg: &Value) -> Result<Document> {
    let response = client(30)?
        .get(url.clone())
        .send()
        .map_err(|e| Error::Fetch(e.without_url().to_string()))?;
    let effective_url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = body(response)?;
    let mut doc = if content_type.contains("html")
        || bytes.starts_with(b"<!DOCTYPE html")
        || bytes.starts_with(b"<html")
    {
        let charset = content_type
            .split(';')
            .find_map(|p| p.trim().strip_prefix("charset="));
        let encoding = charset
            .and_then(|label| encoding_rs::Encoding::for_label(label.trim_matches('"').as_bytes()))
            .unwrap_or(encoding_rs::UTF_8);
        let (html, _, _) = encoding.decode(&bytes);
        formats::extract_html(&html, Some(effective_url.as_str()))?
    } else if content_type.starts_with("text/") && !content_type.contains("xml") {
        Document {
            markdown: String::from_utf8(bytes)
                .map_err(|_| Error::Fetch("Text response is not UTF-8".into()))?,
            ..Default::default()
        }
    } else {
        let extension = if content_type.contains("pdf") {
            "pdf"
        } else {
            std::path::Path::new(effective_url.path())
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        };
        if !formats::supports_extension(extension) {
            return Err(Error::Unsupported(format!(
                "Unsupported URL content type: {content_type}"
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
    Ok(doc)
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
        let doc = fetch(&format!("http://{addr}/page"), &config::defaults()).unwrap();
        server.join().unwrap();
        assert!(doc.markdown.contains("Hello native network."));
        assert_eq!(doc.metadata["fetch_strategy"], "static");
    }
}
