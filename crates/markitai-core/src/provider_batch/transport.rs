use super::input;
use super::types::*;
use reqwest::blocking::{Client as HttpClient, Response, multipart};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;
use url::Url;

/// No environment/config loading, redirects, automatic retries or persisted auth.
pub struct Client {
    http: HttpClient,
    base: Url,
    authorization: HeaderValue,
    limits: Limits,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchClient")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(
        base: &str,
        api_key: &str,
        timeout: Duration,
        limits: Limits,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if base.len() > 8192
            || base.trim() != base
            || base.chars().any(char::is_control)
            || api_key.is_empty()
            || api_key.len() > 8192
            || api_key.chars().any(char::is_control)
            || timeout.is_zero()
            || timeout > Duration::from_secs(1800)
        {
            return Err(Error::Invalid(
                "Batch endpoint, credential or timeout is invalid",
            ));
        }
        let mut base = Url::parse(base).map_err(|_| Error::Invalid("Batch endpoint is invalid"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::Invalid(
                "Batch endpoint must be HTTP(S) without user information, query or fragment",
            ));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| Error::Invalid("Batch credential is invalid"))?;
        authorization.set_sensitive(true);
        let http = HttpClient::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(15)))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            // An implicit proxy is not part of this explicitly supplied identity.
            .no_proxy()
            .gzip(false)
            .brotli(false)
            .build()
            .map_err(|_| Error::Transport)?;
        Ok(Self {
            http,
            base,
            authorization,
            limits,
        })
    }

    fn endpoint(&self, relative: &str) -> Result<Url, Error> {
        self.base
            .join(relative)
            .map_err(|_| Error::Invalid("Batch endpoint is invalid"))
    }

    pub fn upload(&self, source: &Path) -> Result<UploadedInput, Error> {
        self.upload_snapshot(input::snapshot(source, self.limits)?)
    }

    pub(super) fn upload_snapshot(&self, input: input::Input) -> Result<UploadedInput, Error> {
        let input::Input {
            file,
            model,
            custom_ids,
            bytes,
            sha256,
        } = input;
        let part = multipart::Part::reader_with_length(file, bytes)
            .file_name("requests.jsonl")
            .mime_str("application/jsonl")
            .map_err(|_| Error::Invalid("Batch multipart content type is invalid"))?;
        let form = multipart::Form::new()
            .text("purpose", "batch")
            .part("file", part);
        let response = self
            .http
            .post(self.endpoint("files")?)
            .header(AUTHORIZATION, self.authorization.clone())
            .multipart(form)
            .send()
            .map_err(|_| Error::Transport)?;
        let value = self.control(response)?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| identifier(id, "file"))
            .ok_or(Error::Protocol)?;
        if value
            .get("purpose")
            .is_some_and(|purpose| purpose != "batch")
            || value
                .get("bytes")
                .is_some_and(|reported| reported.as_u64() != Some(bytes))
        {
            return Err(Error::Protocol);
        }
        Ok(UploadedInput {
            file_id: id.to_owned(),
            model,
            custom_ids,
            bytes,
            sha256,
        })
    }

    /// Exactly one create attempt. Network/5xx/malformed success cannot prove rejection.
    /// The caller must durably retain UploadedInput and nonce BEFORE this method.
    pub fn create(&self, input: &UploadedInput, nonce: &str) -> Result<Batch, Error> {
        if !identifier(&input.file_id, "file")
            || !custom_id(nonce)
            || input.bytes == 0
            || input.bytes > self.limits.upload_bytes as u64
            || input.custom_ids.is_empty()
            || input.custom_ids.len() > self.limits.requests
            || input.custom_ids.iter().any(|id| !custom_id(id))
            || input.custom_ids.iter().collect::<HashSet<_>>().len() != input.custom_ids.len()
            || input.model.trim().is_empty()
            || input.model.len() > 256
            || input.model.chars().any(char::is_control)
            || input.sha256.len() != 64
            || !input.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Invalid("Batch submission identity is invalid"));
        }
        let url = self.endpoint("batches")?;
        let body = json!({"input_file_id":input.file_id,"endpoint":ENDPOINT,
            "completion_window":"24h","metadata":{"markitai_submission":nonce}});
        let response = self
            .http
            .post(url)
            .header(AUTHORIZATION, self.authorization.clone())
            .json(&body)
            .send()
            .map_err(|_| Error::CreateUncertain)?;
        let status = response.status().as_u16();
        if (400..500).contains(&status) && !matches!(status, 408 | 409) {
            return Err(Error::Http(status));
        }
        if !response.status().is_success() {
            return Err(Error::CreateUncertain);
        }
        let value = self.control(response).map_err(|_| Error::CreateUncertain)?;
        let batch = parse_batch(value).map_err(|_| Error::CreateUncertain)?;
        if batch.input_file_id != input.file_id {
            return Err(Error::CreateUncertain);
        }
        Ok(batch)
    }

    pub fn retrieve(&self, batch_id: &str) -> Result<Batch, Error> {
        if !identifier(batch_id, "batch") {
            return Err(Error::Invalid("Batch ID is invalid"));
        }
        let response = self
            .http
            .get(self.endpoint(&format!("batches/{batch_id}"))?)
            .header(AUTHORIZATION, self.authorization.clone())
            .send()
            .map_err(|_| Error::Transport)?;
        let batch = parse_batch(self.control(response)?)?;
        if batch.id != batch_id {
            return Err(Error::Protocol);
        }
        Ok(batch)
    }

    /// Download both files, including partial results of expired/cancelled jobs.
    /// A failure retains earlier attributed results; the caller owns persistence.
    pub fn download_results(
        &self,
        batch: &Batch,
        expected_ids: &[String],
    ) -> Result<Results, DownloadFailure> {
        let mut collection = match Collection::new(batch.status, expected_ids, self.limits.requests)
        {
            Ok(collection) => collection,
            Err(error) => {
                return Err(DownloadFailure {
                    error,
                    partial: Results {
                        status: batch.status,
                        items: Vec::new(),
                        missing: Vec::new(),
                        bytes: 0,
                    },
                });
            }
        };
        let result = self.download_into(batch, &mut collection);
        let partial = collection.finish();
        match result {
            Ok(()) => Ok(partial),
            Err(error) => Err(DownloadFailure { error, partial }),
        }
    }

    fn download_into(&self, batch: &Batch, collection: &mut Collection<'_>) -> Result<(), Error> {
        if !batch.status.terminal() {
            return Err(Error::Pending);
        }
        if !identifier(&batch.id, "batch")
            || !identifier(&batch.input_file_id, "file")
            || batch
                .output_file_id
                .as_ref()
                .is_some_and(|id| !identifier(id, "file"))
            || batch
                .error_file_id
                .as_ref()
                .is_some_and(|id| !identifier(id, "file"))
            || (batch.output_file_id.is_some() && batch.output_file_id == batch.error_file_id)
        {
            return Err(Error::Protocol);
        }
        for file in [&batch.output_file_id, &batch.error_file_id]
            .into_iter()
            .flatten()
        {
            // Never accept/follow a provider-supplied results_url.
            let response = self
                .http
                .get(self.endpoint(&format!("files/{file}/content"))?)
                .header(AUTHORIZATION, self.authorization.clone())
                .send()
                .map_err(|_| Error::Transport)?;
            if !response.status().is_success() {
                return Err(Error::Http(response.status().as_u16()));
            }
            let remaining = self.limits.result_bytes - collection.bytes;
            if response
                .content_length()
                .is_some_and(|length| length > remaining as u64)
            {
                return Err(Error::Limit(
                    "Batch result files exceed their combined byte limit",
                ));
            }
            let mut response = BufReader::new(response);
            loop {
                let remaining = self.limits.result_bytes - collection.bytes;
                let row = input::line(&mut response, self.limits.line_bytes.min(remaining))?;
                if row.is_empty() {
                    break;
                }
                collection.bytes += row.len();
                let value: Value = serde_json::from_slice(&row).map_err(|_| Error::Protocol)?;
                collection.push(value)?;
            }
        }
        Ok(())
    }

    fn control(&self, response: Response) -> Result<Value, Error> {
        if !response.status().is_success() {
            return Err(Error::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|size| size > self.limits.control_bytes as u64)
        {
            return Err(Error::Limit(
                "Batch control response exceeds its byte limit",
            ));
        }
        let mut bytes = Vec::new();
        response
            .take(self.limits.control_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Transport)?;
        if bytes.len() > self.limits.control_bytes {
            return Err(Error::Limit(
                "Batch control response exceeds its byte limit",
            ));
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
    }
}

fn parse_batch(value: Value) -> Result<Batch, Error> {
    let object = value.as_object().ok_or(Error::Protocol)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| identifier(id, "batch"))
        .ok_or(Error::Protocol)?
        .to_owned();
    let input_file_id = object
        .get("input_file_id")
        .and_then(Value::as_str)
        .filter(|id| identifier(id, "file"))
        .ok_or(Error::Protocol)?
        .to_owned();
    if object
        .get("endpoint")
        .is_some_and(|value| value != ENDPOINT)
    {
        return Err(Error::Protocol);
    }
    let status = serde_json::from_value(object.get("status").cloned().ok_or(Error::Protocol)?)
        .map_err(|_| Error::Protocol)?;
    let file_id = |name| -> Result<Option<String>, Error> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(id)) if identifier(id, "file") => Ok(Some(id.clone())),
            _ => Err(Error::Protocol),
        }
    };
    let count = |name| -> Result<Option<u64>, Error> {
        match object.get("request_counts") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(counts)) => counts
                .get(name)
                .map(|value| value.as_u64().ok_or(Error::Protocol))
                .transpose(),
            _ => Err(Error::Protocol),
        }
    };
    let total = count("total")?;
    let completed = count("completed")?;
    let failed = count("failed")?;
    if let (Some(total), Some(completed), Some(failed)) = (total, completed, failed)
        && completed
            .checked_add(failed)
            .is_none_or(|done| done > total)
    {
        return Err(Error::Protocol);
    }
    Ok(Batch {
        id,
        input_file_id,
        status,
        output_file_id: file_id("output_file_id")?,
        error_file_id: file_id("error_file_id")?,
        total,
        completed,
        failed,
    })
}

struct Collection<'a> {
    status: BatchStatus,
    expected: &'a [String],
    positions: HashMap<&'a str, usize>,
    slots: Vec<Option<ResultItem>>,
    bytes: usize,
}
impl<'a> Collection<'a> {
    fn new(status: BatchStatus, expected: &'a [String], max: usize) -> Result<Self, Error> {
        if expected.is_empty() || expected.len() > max {
            return Err(Error::Invalid("Batch expected request count is invalid"));
        }
        let mut positions = HashMap::with_capacity(expected.len());
        for (index, id) in expected.iter().enumerate() {
            if !custom_id(id) || positions.insert(id.as_str(), index).is_some() {
                return Err(Error::Invalid(
                    "Batch expected IDs are invalid or duplicated",
                ));
            }
        }
        Ok(Self {
            status,
            expected,
            positions,
            slots: vec![None; expected.len()],
            bytes: 0,
        })
    }
    fn push(&mut self, value: Value) -> Result<(), Error> {
        let object = value.as_object().ok_or(Error::Protocol)?;
        let id = object
            .get("custom_id")
            .and_then(Value::as_str)
            .ok_or(Error::Protocol)?;
        let position = *self
            .positions
            .get(id)
            .ok_or(Error::Invalid("Batch result contains an unknown custom_id"))?;
        if self.slots[position].is_some() {
            return Err(Error::Invalid(
                "Batch result contains a duplicate custom_id",
            ));
        }
        let error = object
            .get("error")
            .filter(|value| !value.is_null())
            .cloned();
        if error.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(Error::Protocol);
        }
        let response = object.get("response").filter(|value| !value.is_null());
        let (http_status, body, request_id) = match response {
            Some(Value::Object(response)) => {
                let status = response
                    .get("status_code")
                    .and_then(Value::as_u64)
                    .filter(|code| (100..600).contains(code))
                    .ok_or(Error::Protocol)?;
                let body = response.get("body").filter(|body| !body.is_null()).cloned();
                let request_id = match response.get("request_id") {
                    Some(Value::String(id))
                        if id.len() <= 256 && !id.chars().any(char::is_control) =>
                    {
                        Some(id.clone())
                    }
                    // Optional provider metadata must not erase an attributable paid body.
                    _ => None,
                };
                (Some(status as u16), body, request_id)
            }
            None if error.is_some() => (None, None, None),
            _ => return Err(Error::Protocol),
        };
        self.slots[position] = Some(ResultItem {
            custom_id: id.to_owned(),
            http_status,
            body,
            error,
            request_id,
        });
        Ok(())
    }
    fn finish(self) -> Results {
        let mut items = Vec::new();
        let mut missing = Vec::new();
        for (index, item) in self.slots.into_iter().enumerate() {
            match item {
                Some(item) => items.push(item),
                None => missing.push(self.expected[index].clone()),
            }
        }
        Results {
            status: self.status,
            items,
            missing,
            bytes: self.bytes,
        }
    }
}
