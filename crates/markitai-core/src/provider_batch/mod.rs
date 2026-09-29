//! OpenAI Batch transport only; persistence and output publication belong to callers.
mod input;
mod transport;
mod types;

pub use crate::llm::batch::{DecodedDocument, Endpoint, Plan, Prepared, Session};
pub use transport::Client;
pub use types::{
    Batch, BatchStatus, DownloadFailure, Error, Limits, ResultItem, Results, UploadedInput,
};

#[cfg(test)]
mod tests;
