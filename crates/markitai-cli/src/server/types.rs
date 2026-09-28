use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const MAX_ITEMS: usize = 1000;
pub(super) const MAX_UPLOAD: usize = 100 * 1024 * 1024;
pub(super) const MAX_REQUEST: usize = 5 * 1024 * 1024 * 1024 + 64 * 1024 * 1024;

pub(super) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct JobOptions {
    pub preset: Option<String>,
    pub llm: Option<bool>,
    pub ocr: Option<bool>,
    pub profile: Option<String>,
    pub alt: Option<bool>,
    pub desc: Option<bool>,
    pub screenshot: Option<bool>,
    pub screenshot_only: Option<bool>,
    pub pure: Option<bool>,
    pub no_cache: Option<bool>,
    pub no_compress: Option<bool>,
    pub strategy: Option<String>,
    pub backend: Option<String>,
}

impl JobOptions {
    pub fn config(&self, base: &Value) -> ApiResult<Value> {
        let mut cfg = base.clone();
        if let Some(name) = &self.preset {
            let name = name.to_ascii_lowercase();
            let preset = cfg["presets"].get(&name).cloned().or_else(|| match name.as_str() {
                "minimal" => Some(json!({"llm":false,"ocr":false,"alt":false,"desc":false,"screenshot":false})),
                "standard" => Some(json!({"llm":true,"ocr":false,"alt":true,"desc":true,"screenshot":false})),
                "rich" => Some(json!({"llm":true,"ocr":false,"alt":true,"desc":true,"screenshot":true})),
                _ => None,
            }).ok_or_else(|| ApiError::new(422, format!("unknown preset '{name}'")))?;
            for (key, path) in [
                ("llm", "/llm/enabled"),
                ("ocr", "/ocr/enabled"),
                ("alt", "/image/alt_enabled"),
                ("desc", "/image/desc_enabled"),
                ("screenshot", "/screenshot/enabled"),
            ] {
                if let Some(value) = preset.get(key) {
                    *cfg.pointer_mut(path).unwrap() = value.clone();
                }
            }
        }
        for (path, value) in [
            ("/llm/enabled", self.llm),
            ("/ocr/enabled", self.ocr),
            ("/image/alt_enabled", self.alt),
            ("/image/desc_enabled", self.desc),
            ("/screenshot/enabled", self.screenshot),
            ("/screenshot/screenshot_only", self.screenshot_only),
            ("/llm/pure", self.pure),
            ("/cache/no_cache", self.no_cache),
        ] {
            if let Some(value) = value {
                *cfg.pointer_mut(path).unwrap() = json!(value);
            }
        }
        if self.screenshot_only == Some(true) {
            cfg["screenshot"]["enabled"] = json!(true);
        }
        if let Some(value) = self.no_compress {
            cfg["image"]["compress"] = json!(!value);
        }
        if let Some(value) = &self.profile {
            cfg["output"]["profile"] = json!(value);
        }
        if let Some(value) = &self.strategy {
            cfg["fetch"]["strategy"] = json!(value);
        }
        if let Some(value) = &self.backend {
            if !["native", "cloudflare"].contains(&value.as_str()) {
                return Err(ApiError::new(422, "invalid backend"));
            }
            cfg["fetch"]["cloudflare"]["convert_enabled"] = json!(value == "cloudflare");
        }
        if cfg["llm"]["enabled"] == true {
            cfg["llm"]["keep_base"] = json!(true);
        }
        // A service never waits for terminal input or writes user-selected output paths.
        if cfg["fetch"]["remote_consent"] == "ask" {
            cfg["fetch"]["remote_consent"] = json!("never");
        }
        cfg["history"]["record"] = json!(false);
        if let Some(output) = cfg["output"].as_object_mut() {
            output.remove("filename");
            output.remove("reserved_stem");
        }
        cfg["output"]["allow_symlinks"] = json!(false);
        markitai_core::config::normalize(&cfg)
            .map_err(|error| ApiError::new(422, error.to_string()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Item {
    pub item_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub error: Option<String>,
    pub output: Option<String>,
    pub output_name: Option<String>,
    pub duration_ms: Option<u64>,
    pub finished_at: Option<String>,
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub llm_enhanced: bool,
    #[serde(default = "convert_operation")]
    pub operation: String,
    #[serde(default)]
    pub skipped: bool,
    #[serde(default)]
    pub skip_reason: Option<String>,
    #[serde(default = "yes")]
    pub retryable: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}
fn convert_operation() -> String {
    "convert".into()
}
fn yes() -> bool {
    true
}
impl Item {
    pub fn new(index: usize, name: String, kind: &str, output_name: Option<String>) -> Self {
        Self {
            item_id: format!("i{index}"),
            name,
            kind: kind.into(),
            status: "queued".into(),
            error: None,
            output: None,
            output_name,
            duration_ms: None,
            finished_at: None,
            cost_usd: None,
            llm_enhanced: false,
            operation: "convert".into(),
            skipped: false,
            skip_reason: None,
            retryable: true,
            warnings: Vec::new(),
        }
    }
    pub fn created(&self) -> Value {
        json!({"item_id":self.item_id,"name":self.name,"kind":self.kind})
    }
}

pub(super) type ApiResult<T> = Result<T, ApiError>;
pub(super) struct ApiError {
    pub status: StatusCode,
    pub detail: String,
}
impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail: detail.into(),
        }
    }
    pub fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("Serve: {error}");
        Self::new(500, "internal server error")
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self.status.as_u16() {
            400 => "bad_request",
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not_found",
            405 => "method_not_allowed",
            409 => "conflict",
            413 => "payload_too_large",
            422 => "invalid_request",
            429 => "rate_limited",
            503 => "unavailable",
            _ => "server_error",
        };
        let mut response =
            (self.status, Json(json!({"detail":self.detail,"code":code}))).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert("www-authenticate", "Bearer".parse().unwrap());
        }
        response
    }
}
