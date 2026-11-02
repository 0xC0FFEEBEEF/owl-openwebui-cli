use codex_client::TransportError;
use http::StatusCode;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("api error {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("stream error: {0}")]
    Stream(String),
    #[error("context window exceeded")]
    ContextWindowExceeded,
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("retryable error: {message}")]
    Retryable {
        message: String,
        delay: Option<Duration>,
    },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
}
