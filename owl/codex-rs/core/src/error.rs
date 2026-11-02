use crate::exec::ExecToolCallOutput;
use crate::truncate::TruncationPolicy;
use crate::truncate::truncate_text;
use codex_async_utils::CancelErr;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ErrorEvent;
use reqwest::StatusCode;
use serde_json;
use std::io;
use std::time::Duration;
use thiserror::Error;
use tokio::task::JoinError;

pub type Result<T> = std::result::Result<T, CodexErr>;

/// Limit UI error messages to a reasonable size while keeping useful context.
const ERROR_MESSAGE_UI_MAX_BYTES: usize = 2 * 1024; // 2 KiB

#[derive(Error, Debug)]
pub enum SandboxErr {
    /// Error from sandbox execution
    #[error(
        "sandbox denied exec error, exit code: {}, stdout: {}, stderr: {}",
        .output.exit_code, .output.stdout.text, .output.stderr.text
    )]
    Denied { output: Box<ExecToolCallOutput> },

    /// Error from linux seccomp filter setup
    #[cfg(target_os = "linux")]
    #[error("seccomp setup error")]
    SeccompInstall(#[from] seccompiler::Error),

    /// Error from linux seccomp backend
    #[cfg(target_os = "linux")]
    #[error("seccomp backend error")]
    SeccompBackend(#[from] seccompiler::BackendError),

    /// Command timed out
    #[error("command timed out")]
    Timeout { output: Box<ExecToolCallOutput> },

    /// Command was killed by a signal
    #[error("command was killed by a signal")]
    Signal(i32),

    /// Error from linux landlock
    #[error("Landlock was not able to fully enforce all sandbox rules")]
    LandlockRestrict,
}

#[derive(Error, Debug)]
pub enum CodexErr {
    #[error("turn aborted. Something went wrong? Hit `/feedback` to report the issue.")]
    TurnAborted,

    /// Returned by ResponsesClient when the SSE stream disconnects or errors out **after** the HTTP
    /// handshake has succeeded but **before** it finished emitting `response.completed`.
    ///
    /// The Session loop treats this as a transient error and will automatically retry the turn.
    ///
    /// Optionally includes the requested delay before retrying the turn.
    #[error("stream disconnected before completion: {0}")]
    Stream(String, Option<Duration>),

    #[error(
        "owl ran out of room in the model's context window. Start a new conversation or clear earlier history before retrying."
    )]
    ContextWindowExceeded,

    #[error("no thread with id: {0}")]
    ThreadNotFound(ThreadId),

    #[error("agent thread limit reached (max {max_threads})")]
    AgentLimitReached { max_threads: usize },

    #[error("session configured event was not the first event in the stream")]
    SessionConfiguredNotFirstEvent,

    /// Returned by run_command_stream when the spawned child process timed out (10s).
    #[error("timeout waiting for child process to exit")]
    Timeout,

    /// Returned by run_command_stream when the child could not be spawned (its stdout/stderr pipes
    /// could not be captured). Analogous to the previous `CodexError::Spawn` variant.
    #[error("spawn failed: child stdout/stderr not captured")]
    Spawn,

    /// Returned by run_command_stream when the user pressed Ctrl‑C (SIGINT). Session uses this to
    /// surface a polite FunctionCallOutput back to the model instead of crashing the CLI.
    #[error("interrupted (Ctrl-C). Something went wrong? Hit `/feedback` to report the issue.")]
    Interrupted,

    /// Unexpected HTTP status code.
    #[error("{0}")]
    UnexpectedStatus(UnexpectedResponseError),

    /// Invalid request.
    #[error("{0}")]
    InvalidRequest(String),

    /// Invalid image.
    #[error("Image poisoning")]
    InvalidImageRequest(),

    #[error("{0}")]
    ResponseStreamFailed(ResponseStreamFailed),

    #[error("{0}")]
    ConnectionFailed(ConnectionFailedError),

    #[error("Your server reported a quota limit for this model.")]
    QuotaExceeded,

    #[error("We're currently experiencing high demand, which may cause temporary errors.")]
    InternalServerError,

    /// Retry limit exceeded.
    #[error("{0}")]
    RetryLimit(RetryLimitReachedError),

    /// Agent loop died unexpectedly
    #[error("internal error; agent loop died unexpectedly")]
    InternalAgentDied,

    /// Sandbox error
    #[error("sandbox error: {0}")]
    Sandbox(#[from] SandboxErr),

    #[error("codex-linux-sandbox was required but not provided")]
    LandlockSandboxExecutableNotProvided,

    #[error("unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Fatal error: {0}")]
    Fatal(String),

    // -----------------------------------------------------------------
    // Automatic conversions for common external error types
    // -----------------------------------------------------------------
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[cfg(target_os = "linux")]
    #[error(transparent)]
    LandlockRuleset(#[from] landlock::RulesetError),

    #[cfg(target_os = "linux")]
    #[error(transparent)]
    LandlockPathFd(#[from] landlock::PathFdError),

    #[error(transparent)]
    TokioJoin(#[from] JoinError),

    #[error("{0}")]
    EnvVar(EnvVarError),
}

impl From<CancelErr> for CodexErr {
    fn from(_: CancelErr) -> Self {
        CodexErr::TurnAborted
    }
}

impl CodexErr {
    pub fn is_retryable(&self) -> bool {
        match self {
            CodexErr::TurnAborted
            | CodexErr::Interrupted
            | CodexErr::EnvVar(_)
            | CodexErr::Fatal(_)
            | CodexErr::QuotaExceeded
            | CodexErr::InvalidImageRequest()
            | CodexErr::InvalidRequest(_)
            | CodexErr::UnsupportedOperation(_)
            | CodexErr::Sandbox(_)
            | CodexErr::LandlockSandboxExecutableNotProvided
            | CodexErr::RetryLimit(_)
            | CodexErr::ContextWindowExceeded
            | CodexErr::ThreadNotFound(_)
            | CodexErr::AgentLimitReached { .. }
            | CodexErr::Spawn
            | CodexErr::SessionConfiguredNotFirstEvent => false,
            CodexErr::Stream(..)
            | CodexErr::Timeout
            | CodexErr::UnexpectedStatus(_)
            | CodexErr::ResponseStreamFailed(_)
            | CodexErr::ConnectionFailed(_)
            | CodexErr::InternalServerError
            | CodexErr::InternalAgentDied
            | CodexErr::Io(_)
            | CodexErr::Json(_)
            | CodexErr::TokioJoin(_) => true,
            #[cfg(target_os = "linux")]
            CodexErr::LandlockRuleset(_) | CodexErr::LandlockPathFd(_) => false,
        }
    }
}

#[derive(Debug)]
pub struct ConnectionFailedError {
    pub source: reqwest::Error,
}

impl std::fmt::Display for ConnectionFailedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Connection failed: {}", self.source)
    }
}

#[derive(Debug)]
pub struct ResponseStreamFailed {
    pub source: reqwest::Error,
    pub request_id: Option<String>,
}

impl std::fmt::Display for ResponseStreamFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error while reading the server response: {}{}",
            self.source,
            self.request_id
                .as_ref()
                .map(|id| format!(", request id: {id}"))
                .unwrap_or_default()
        )
    }
}

#[derive(Debug)]
pub struct UnexpectedResponseError {
    pub status: StatusCode,
    pub body: String,
    pub url: Option<String>,
    pub request_id: Option<String>,
}

const CLOUDFLARE_BLOCKED_MESSAGE: &str =
    "Access blocked by Cloudflare. This usually happens when connecting from a restricted region";

impl UnexpectedResponseError {
    fn friendly_message(&self) -> Option<String> {
        if self.status != StatusCode::FORBIDDEN {
            return None;
        }

        if !self.body.contains("Cloudflare") || !self.body.contains("blocked") {
            return None;
        }

        let status = self.status;
        let mut message = format!("{CLOUDFLARE_BLOCKED_MESSAGE} (status {status})");
        if let Some(url) = &self.url {
            message.push_str(&format!(", url: {url}"));
        }
        if let Some(id) = &self.request_id {
            message.push_str(&format!(", request id: {id}"));
        }

        Some(message)
    }
}

impl std::fmt::Display for UnexpectedResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(friendly) = self.friendly_message() {
            write!(f, "{friendly}")
        } else {
            let status = self.status;
            let body = &self.body;
            let mut message = format!("unexpected status {status}: {body}");
            if let Some(url) = &self.url {
                message.push_str(&format!(", url: {url}"));
            }
            if let Some(id) = &self.request_id {
                message.push_str(&format!(", request id: {id}"));
            }
            write!(f, "{message}")
        }
    }
}

impl std::error::Error for UnexpectedResponseError {}
#[derive(Debug)]
pub struct RetryLimitReachedError {
    pub status: StatusCode,
    pub request_id: Option<String>,
}

impl std::fmt::Display for RetryLimitReachedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exceeded retry limit, last status: {}{}",
            self.status,
            self.request_id
                .as_ref()
                .map(|id| format!(", request id: {id}"))
                .unwrap_or_default()
        )
    }
}

#[derive(Debug)]
pub struct EnvVarError {
    /// Name of the environment variable that is missing.
    pub var: String,

    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub instructions: Option<String>,
}

impl std::fmt::Display for EnvVarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing environment variable: `{}`.", self.var)?;
        if let Some(instructions) = &self.instructions {
            write!(f, " {instructions}")?;
        }
        Ok(())
    }
}

impl CodexErr {
    /// Minimal shim so that existing `e.downcast_ref::<CodexErr>()` checks continue to compile
    /// after replacing `anyhow::Error` in the return signature. This mirrors the behavior of
    /// `anyhow::Error::downcast_ref` but works directly on our concrete enum.
    pub fn downcast_ref<T: std::any::Any>(&self) -> Option<&T> {
        (self as &dyn std::any::Any).downcast_ref::<T>()
    }

    /// Translate core error to client-facing protocol error.
    pub fn to_codex_protocol_error(&self) -> CodexErrorInfo {
        match self {
            CodexErr::ContextWindowExceeded => CodexErrorInfo::ContextWindowExceeded,
            CodexErr::QuotaExceeded => CodexErrorInfo::UsageLimitExceeded,
            CodexErr::RetryLimit(_) => CodexErrorInfo::ResponseTooManyFailedAttempts {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::ConnectionFailed(_) => CodexErrorInfo::HttpConnectionFailed {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::ResponseStreamFailed(_) => CodexErrorInfo::ResponseStreamConnectionFailed {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::SessionConfiguredNotFirstEvent
            | CodexErr::InternalServerError
            | CodexErr::InternalAgentDied => CodexErrorInfo::InternalServerError,
            CodexErr::UnsupportedOperation(_)
            | CodexErr::ThreadNotFound(_)
            | CodexErr::AgentLimitReached { .. } => CodexErrorInfo::BadRequest,
            CodexErr::Sandbox(_) => CodexErrorInfo::SandboxError,
            _ => CodexErrorInfo::Other,
        }
    }

    pub fn to_error_event(&self, message_prefix: Option<String>) -> ErrorEvent {
        let error_message = self.to_string();
        let message: String = match message_prefix {
            Some(prefix) => format!("{prefix}: {error_message}"),
            None => error_message,
        };
        ErrorEvent {
            message,
            codex_error_info: Some(self.to_codex_protocol_error()),
        }
    }

    pub fn http_status_code_value(&self) -> Option<u16> {
        let http_status_code = match self {
            CodexErr::RetryLimit(err) => Some(err.status),
            CodexErr::UnexpectedStatus(err) => Some(err.status),
            CodexErr::ConnectionFailed(err) => err.source.status(),
            CodexErr::ResponseStreamFailed(err) => err.source.status(),
            _ => None,
        };
        http_status_code.as_ref().map(StatusCode::as_u16)
    }
}

pub fn get_error_message_ui(e: &CodexErr) -> String {
    let message = match e {
        CodexErr::Sandbox(SandboxErr::Denied { output }) => {
            let aggregated = output.aggregated_output.text.trim();
            if !aggregated.is_empty() {
                output.aggregated_output.text.clone()
            } else {
                let stderr = output.stderr.text.trim();
                let stdout = output.stdout.text.trim();
                match (stderr.is_empty(), stdout.is_empty()) {
                    (false, false) => format!("{stderr}\n{stdout}"),
                    (false, true) => output.stderr.text.clone(),
                    (true, false) => output.stdout.text.clone(),
                    (true, true) => format!(
                        "command failed inside sandbox with exit code {}",
                        output.exit_code
                    ),
                }
            }
        }
        // Timeouts are not sandbox errors from a UX perspective; present them plainly
        CodexErr::Sandbox(SandboxErr::Timeout { output }) => {
            format!(
                "error: command timed out after {} ms",
                output.duration.as_millis()
            )
        }
        _ => e.to_string(),
    };

    truncate_text(
        &message,
        TruncationPolicy::Bytes(ERROR_MESSAGE_UI_MAX_BYTES),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::StreamOutput;
    use pretty_assertions::assert_eq;
    use reqwest::Response;
    use reqwest::ResponseBuilderExt;
    use reqwest::StatusCode;
    use reqwest::Url;

    #[test]
    fn sandbox_denied_uses_aggregated_output_when_stderr_empty() {
        let output = ExecToolCallOutput {
            exit_code: 77,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new("aggregate detail".to_string()),
            duration: Duration::from_millis(10),
            timed_out: false,
        };
        let err = CodexErr::Sandbox(SandboxErr::Denied {
            output: Box::new(output),
        });
        assert_eq!(get_error_message_ui(&err), "aggregate detail");
    }

    #[test]
    fn sandbox_denied_reports_both_streams_when_available() {
        let output = ExecToolCallOutput {
            exit_code: 9,
            stdout: StreamOutput::new("stdout detail".to_string()),
            stderr: StreamOutput::new("stderr detail".to_string()),
            aggregated_output: StreamOutput::new(String::new()),
            duration: Duration::from_millis(10),
            timed_out: false,
        };
        let err = CodexErr::Sandbox(SandboxErr::Denied {
            output: Box::new(output),
        });
        assert_eq!(get_error_message_ui(&err), "stderr detail\nstdout detail");
    }

    #[test]
    fn sandbox_denied_reports_stdout_when_no_stderr() {
        let output = ExecToolCallOutput {
            exit_code: 11,
            stdout: StreamOutput::new("stdout only".to_string()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new(String::new()),
            duration: Duration::from_millis(8),
            timed_out: false,
        };
        let err = CodexErr::Sandbox(SandboxErr::Denied {
            output: Box::new(output),
        });
        assert_eq!(get_error_message_ui(&err), "stdout only");
    }

    #[test]
    fn to_error_event_handles_response_stream_failed() {
        let response = http::Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .url(Url::parse("http://example.com").unwrap())
            .body("")
            .unwrap();
        let source = Response::from(response).error_for_status_ref().unwrap_err();
        let err = CodexErr::ResponseStreamFailed(ResponseStreamFailed {
            source,
            request_id: Some("req-123".to_string()),
        });

        let event = err.to_error_event(Some("prefix".to_string()));

        assert_eq!(
            event.message,
            "prefix: Error while reading the server response: HTTP status client error (429 Too Many Requests) for url (http://example.com/), request id: req-123"
        );
        assert_eq!(
            event.codex_error_info,
            Some(CodexErrorInfo::ResponseStreamConnectionFailed {
                http_status_code: Some(429)
            })
        );
    }

    #[test]
    fn sandbox_denied_reports_exit_code_when_no_output_available() {
        let output = ExecToolCallOutput {
            exit_code: 13,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new(String::new()),
            duration: Duration::from_millis(5),
            timed_out: false,
        };
        let err = CodexErr::Sandbox(SandboxErr::Denied {
            output: Box::new(output),
        });
        assert_eq!(
            get_error_message_ui(&err),
            "command failed inside sandbox with exit code 13"
        );
    }

    #[test]
    fn unexpected_status_cloudflare_html_is_simplified() {
        let err = UnexpectedResponseError {
            status: StatusCode::FORBIDDEN,
            body: "<html><body>Cloudflare error: Sorry, you have been blocked</body></html>"
                .to_string(),
            url: Some("http://example.com/blocked".to_string()),
            request_id: Some("ray-id".to_string()),
        };
        let status = StatusCode::FORBIDDEN.to_string();
        let url = "http://example.com/blocked";
        assert_eq!(
            err.to_string(),
            format!(
                "{CLOUDFLARE_BLOCKED_MESSAGE} (status {status}), url: {url}, request id: ray-id"
            )
        );
    }

    #[test]
    fn unexpected_status_non_html_is_unchanged() {
        let err = UnexpectedResponseError {
            status: StatusCode::FORBIDDEN,
            body: "plain text error".to_string(),
            url: Some("http://example.com/plain".to_string()),
            request_id: None,
        };
        let status = StatusCode::FORBIDDEN.to_string();
        let url = "http://example.com/plain";
        assert_eq!(
            err.to_string(),
            format!("unexpected status {status}: plain text error, url: {url}")
        );
    }
}
