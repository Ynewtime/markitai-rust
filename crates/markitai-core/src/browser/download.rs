//! Main-navigation response classification and bounded CDP stream decoding.
use super::BrowserPdf;
use crate::{Error, Result};
use base64::Engine;
use serde_json::{Value, json};
use url::Url;

pub(super) const LIMIT: usize = 100 * 1024 * 1024;
pub(super) const CHUNK: usize = 64 * 1024;

pub(super) struct Response {
    pub id: String,
    pub url: String,
    pub status: u64,
    pub headers: Vec<Value>,
    pdf_mime: bool,
    expected_length: Option<usize>,
}

fn failure(message: &str) -> Error {
    Error::Fetch(message.into())
}

impl Response {
    pub fn candidate(params: &Value) -> Result<Option<Self>> {
        let status = params["responseStatusCode"].as_u64().unwrap_or(0);
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        let headers = params["responseHeaders"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if headers.len() > 2048
            || headers
                .iter()
                .map(|v| {
                    v["name"]
                        .as_str()
                        .unwrap_or("")
                        .len()
                        .saturating_add(v["value"].as_str().unwrap_or("").len())
                })
                .sum::<usize>()
                > 128 * 1024
        {
            return Err(failure(
                "Browser response headers exceed the download limit",
            ));
        }
        let header = |key: &str| {
            headers
                .iter()
                .find(|h| {
                    h["name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(key))
                })
                .and_then(|h| h["value"].as_str())
        };
        let mime = header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // A text representation can contain PDF examples without becoming a PDF.
        if mime.starts_with("text/") || mime.contains("html") {
            return Ok(None);
        }
        let url = params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("Browser document response has no URL"))?;
        super::final_http_url(url)?;
        let pdf_mime = matches!(mime.as_str(), "application/pdf" | "application/x-pdf");
        let generic = matches!(
            mime.as_str(),
            "" | "application/octet-stream"
                | "binary/octet-stream"
                | "application/binary"
                | "application/download"
        );
        let path_hint = Url::parse(url).ok().is_some_and(|url| {
            std::path::Path::new(url.path())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        });
        if !pdf_mime && !generic && !path_hint {
            return Ok(None);
        }
        if status == 206 {
            return Err(failure("Browser PDF download returned a partial response"));
        }
        if let Some(size) =
            header("content-length").and_then(|value| value.trim().parse::<u64>().ok())
            && size > LIMIT as u64
        {
            return Err(failure("Browser document download exceeds 100 MiB"));
        }
        let expected_length = if header("content-encoding")
            .is_none_or(|value| value.trim().eq_ignore_ascii_case("identity"))
        {
            header("content-length").and_then(|value| value.trim().parse::<usize>().ok())
        } else {
            None
        };
        let id = params["requestId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 4096)
            .ok_or_else(|| failure("Malformed Chromium download request"))?;
        Ok(Some(Self {
            id: id.into(),
            url: url.into(),
            status,
            headers,
            pdf_mime,
            expected_length,
        }))
    }

    pub fn validate_length(&self, bytes: &[u8]) -> Result<()> {
        if self.expected_length.is_some_and(|size| size != bytes.len()) {
            return Err(failure("Browser document download was incomplete"));
        }
        Ok(())
    }

    pub fn is_pdf(&self, bytes: &[u8]) -> bool {
        self.pdf_mime
            || bytes[..bytes.len().min(1024)].windows(8).any(|s| {
                &s[..5] == b"%PDF-"
                    && matches!(s[5], b'1' | b'2')
                    && s[6] == b'.'
                    && s[7].is_ascii_digit()
            })
    }

    pub fn pdf(self, bytes: Vec<u8>) -> BrowserPdf {
        BrowserPdf {
            bytes,
            final_url: self.url,
            warnings: Vec::new(),
        }
    }

    pub fn fulfilled(&self, bytes: &[u8]) -> Value {
        // IO streams contain decoded bytes; wire compression/framing headers no
        // longer describe the replacement body. No network request is repeated.
        let mut headers = self
            .headers
            .iter()
            .filter(|header| {
                !header["name"].as_str().is_some_and(|name| {
                    ["content-encoding", "content-length", "transfer-encoding"]
                        .iter()
                        .any(|key| name.eq_ignore_ascii_case(key))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        headers.push(json!({"name":"Content-Length","value":bytes.len().to_string()}));
        json!({"requestId":self.id,"responseCode":self.status,"responseHeaders":headers,"body":base64::engine::general_purpose::STANDARD.encode(bytes)})
    }
}

pub(super) fn append_chunk(bytes: &mut Vec<u8>, value: &Value) -> Result<bool> {
    let data = value["data"]
        .as_str()
        .ok_or_else(|| failure("Chromium download stream returned no bytes"))?;
    if data.len() > CHUNK * 2 {
        return Err(failure("Chromium download stream exceeded its chunk bound"));
    }
    let chunk = if value["base64Encoded"].as_bool() == Some(true) {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| failure("Chromium download stream returned invalid encoding"))?
    } else {
        data.as_bytes().to_vec()
    };
    if chunk.len() > CHUNK
        || bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > LIMIT)
    {
        return Err(failure("Browser document download exceeds 100 MiB"));
    }
    let eof = value["eof"]
        .as_bool()
        .ok_or_else(|| failure("Chromium download stream has no completion state"))?;
    if chunk.is_empty() && !eof {
        return Err(failure("Chromium download stream made no progress"));
    }
    bytes.extend_from_slice(&chunk);
    Ok(eof)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(mime: &str, url: &str, status: u64, size: usize) -> Value {
        json!({"requestId":"response","request":{"url":url},"responseStatusCode":status,"responseHeaders":[{"name":"Content-Type","value":mime},{"name":"Content-Length","value":size.to_string()}]})
    }
    #[test]
    fn text_priority_magic_and_http_boundaries_do_not_confuse_binary_examples() {
        let pdf = b"%PDF-1.4\nbody";
        for mime in ["text/html", "text/plain", "application/xhtml+xml"] {
            assert!(
                Response::candidate(&response(
                    mime,
                    "https://example.test/a.pdf",
                    200,
                    pdf.len()
                ))
                .unwrap()
                .is_none()
            );
        }
        let binary = Response::candidate(&response(
            "application/octet-stream",
            "https://example.test/download",
            200,
            pdf.len(),
        ))
        .unwrap()
        .unwrap();
        assert!(binary.is_pdf(pdf));
        assert!(!binary.is_pdf(b"normal HTML or some unrelated binary"));
        let declared = Response::candidate(&response(
            "application/pdf",
            "https://example.test/download",
            200,
            4,
        ))
        .unwrap()
        .unwrap();
        assert!(declared.is_pdf(b"oops"));
        assert!(declared.validate_length(b"oops").is_ok());
        assert!(declared.validate_length(b"oo").is_err());
        for status in [301, 401, 403, 404, 500] {
            assert!(
                Response::candidate(&response(
                    "application/pdf",
                    "https://example.test/file",
                    status,
                    1
                ))
                .unwrap()
                .is_none()
            );
        }
        assert!(
            Response::candidate(&response(
                "application/pdf",
                "https://example.test/file",
                206,
                1
            ))
            .is_err()
        );
        assert!(
            Response::candidate(&response(
                "application/pdf",
                "https://example.test/file",
                200,
                LIMIT + 1
            ))
            .is_err()
        );
    }
    #[test]
    fn non_pdf_replay_keeps_representation_without_wire_encoding_headers() {
        let mut event = response(
            "application/octet-stream",
            "https://example.test/download",
            200,
            1,
        );
        event["responseHeaders"].as_array_mut().unwrap().extend([
            json!({"name":"Content-Encoding","value":"gzip"}),
            json!({"name":"Content-Security-Policy","value":"default-src 'none'"}),
        ]);
        let parsed = Response::candidate(&event).unwrap().unwrap();
        let replay = parsed.fulfilled(b"<html>body</html>");
        let headers = replay["responseHeaders"].as_array().unwrap();
        assert!(
            headers
                .iter()
                .any(|header| header["name"] == "Content-Security-Policy")
        );
        assert!(
            !headers
                .iter()
                .any(|header| header["name"] == "Content-Encoding")
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(replay["body"].as_str().unwrap())
                .unwrap(),
            b"<html>body</html>"
        );
    }
    #[test]
    fn stream_chunks_preserve_binary_and_reject_no_progress_or_oversized_values() {
        let mut bytes = Vec::new();
        let chunk = [0, 128, 255, b'%'];
        assert!(!append_chunk(&mut bytes,&json!({"data":base64::engine::general_purpose::STANDARD.encode(chunk),"base64Encoded":true,"eof":false})).unwrap());
        assert!(append_chunk(&mut bytes, &json!({"data":"PDF","eof":true})).unwrap());
        assert_eq!(bytes, [0, 128, 255, b'%', b'P', b'D', b'F']);
        for bad in [
            json!({"data":"","eof":false}),
            json!({"data":"!bad","base64Encoded":true,"eof":true}),
            json!({"data":"x".repeat(CHUNK+1),"eof":true}),
        ] {
            assert!(append_chunk(&mut Vec::new(), &bad).is_err());
        }
    }
}
