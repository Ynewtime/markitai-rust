use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Input file not found: {0}")]
    NotFound(String),
    #[error("Input is a directory: {0}")]
    IsDirectory(String),
    #[error("No model configured; set MODEL and a provider API key, or llm.model_list")]
    NoModelConfigured,
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Conversion(String),
    #[error("{0}")]
    ImageOnly(String),
    #[error("{0}")]
    Fetch(String),
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::IsDirectory(_) => "is_directory",
            Self::NoModelConfigured => "no_model_configured",
            Self::InvalidInput(_) => "invalid_input",
            Self::Unsupported(_) => "unsupported",
            Self::Conversion(_) | Self::ImageOnly(_) => "conversion_error",
            Self::Fetch(_) => "fetch_error",
            Self::Config(_) => "config_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "invalid_json",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A failed conversion's existing error category and already recorded model usage.
/// Costs remain subject to the same pricing limitations as successful results.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct ConversionFailure {
    #[source]
    pub error: Error,
    pub usage: ConversionUsage,
}

impl From<Error> for ConversionFailure {
    fn from(error: Error) -> Self {
        Self {
            error,
            usage: ConversionUsage::default(),
        }
    }
}

impl ConversionFailure {
    pub fn code(&self) -> &'static str {
        self.error.code()
    }
}

pub type DetailedResult<T> = std::result::Result<T, ConversionFailure>;

#[derive(Clone, Debug, Serialize)]
pub struct LlmCapabilities {
    pub configured: bool,
    pub routable: bool,
    pub effective: bool,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConvertOptions {
    pub output_dir: Option<PathBuf>,
    pub config: Option<Value>,
    pub llm: Option<bool>,
    pub ocr: Option<bool>,
    pub screenshot: Option<bool>,
    pub alt: Option<bool>,
    pub desc: Option<bool>,
    pub profile: Option<String>,
}

/// Native run context that is not part of the host JSON protocol.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ConvertContext<'a> {
    pub explicit_fetch_strategy: Option<&'a str>,
    pub llm_runtime: Option<&'a crate::LlmRuntime>,
    pub browser_runtime: Option<&'a crate::BrowserRuntime>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub source: String,
    #[serde(default)]
    pub options: ConvertOptions,
}

#[derive(Debug, Default)]
pub struct Document {
    pub markdown: String,
    pub metadata: Map<String, Value>,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct Asset {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversionUsage {
    pub cost_usd: f64,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub by_model: Map<String, Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConversionOutput {
    pub source: String,
    pub markdown: String,
    pub llm_markdown: Option<String>,
    pub frontmatter: Map<String, Value>,
    pub output_path: Option<PathBuf>,
    pub llm_output_path: Option<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub screenshots: Vec<PathBuf>,
    pub images: Vec<Value>,
    pub usage: ConversionUsage,
    pub skip_reason: Option<String>,
    pub duration: f64,
    pub warnings: Vec<String>,
    /// Original YAML bytes retained for writing pure input without reformatting.
    #[serde(skip)]
    pub(crate) pure_prefix: Option<String>,
    #[serde(skip)]
    pub(crate) pure_llm_prefix: Option<String>,
    #[serde(skip)]
    pub(crate) base_frontmatter: Option<Map<String, Value>>,
    #[serde(skip)]
    pub(crate) llm_cache_hit: bool,
    #[serde(skip)]
    pub(crate) fetch_cache_hit: bool,
    #[serde(skip)]
    pub(crate) fetch_strategy: Option<String>,
}

impl ConversionOutput {
    /// Whether this conversion reused an existing document enhancement.
    /// Host JSON remains compatible; the CLI has separate cache-status fields.
    pub fn llm_cache_hit(&self) -> bool {
        self.llm_cache_hit
    }

    /// Whether a fetched page was reused directly or after HTTP validation.
    pub fn fetch_cache_hit(&self) -> bool {
        self.fetch_cache_hit
    }

    /// The strategy actually used to fetch a URL, including pure output mode.
    pub fn fetch_strategy(&self) -> Option<&str> {
        self.fetch_strategy.as_deref().or_else(|| {
            self.frontmatter
                .get("fetch_strategy")
                .and_then(Value::as_str)
        })
    }
}
