use codex_api::AuthProvider as ApiAuthProvider;
use codex_api::TransportError;
use codex_api::error::ApiError;
use http::HeaderMap;

use crate::auth::CodexAuth;
use crate::error::CodexErr;
use crate::error::RetryLimitReachedError;
use crate::error::UnexpectedResponseError;
use crate::model_provider_info::ModelProviderInfo;

pub(crate) fn map_api_error(err: ApiError) -> CodexErr {
    match err {
        ApiError::ContextWindowExceeded => CodexErr::ContextWindowExceeded,
        ApiError::QuotaExceeded => CodexErr::QuotaExceeded,
        ApiError::Retryable { message, delay } => CodexErr::Stream(message, delay),
        ApiError::Stream(msg) => CodexErr::Stream(msg, None),
        ApiError::Api { status, message } => CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status,
            body: message,
            url: None,
            request_id: None,
        }),
        ApiError::InvalidRequest { message } => CodexErr::InvalidRequest(message),
        ApiError::Transport(transport) => match transport {
            TransportError::Http {
                status,
                url,
                headers,
                body,
            } => {
                let body_text = body.unwrap_or_default();

                if status == http::StatusCode::BAD_REQUEST {
                    if body_text
                        .contains("The image data you provided does not represent a valid image")
                    {
                        CodexErr::InvalidImageRequest()
                    } else {
                        CodexErr::InvalidRequest(body_text)
                    }
                } else if status == http::StatusCode::INTERNAL_SERVER_ERROR {
                    CodexErr::InternalServerError
                } else if status == http::StatusCode::TOO_MANY_REQUESTS {
                    // Rate-limited by the user's server: retry within the
                    // provider's configured budget.
                    CodexErr::RetryLimit(RetryLimitReachedError {
                        status,
                        request_id: extract_request_id(headers.as_ref()),
                    })
                } else {
                    CodexErr::UnexpectedStatus(UnexpectedResponseError {
                        status,
                        body: body_text,
                        url,
                        request_id: extract_request_id(headers.as_ref()),
                    })
                }
            }
            TransportError::RetryLimit => CodexErr::RetryLimit(RetryLimitReachedError {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                request_id: None,
            }),
            TransportError::Timeout => CodexErr::Timeout,
            TransportError::Network(msg) | TransportError::Build(msg) => {
                CodexErr::Stream(msg, None)
            }
        },
    }
}

fn extract_request_id(headers: Option<&HeaderMap>) -> Option<String> {
    headers.and_then(|map| {
        ["cf-ray", "x-request-id", "x-oai-request-id"]
            .iter()
            .find_map(|name| {
                map.get(*name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            })
    })
}

pub(crate) fn auth_provider_from_auth(
    auth: Option<CodexAuth>,
    provider: &ModelProviderInfo,
) -> crate::error::Result<CoreAuthProvider> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(CoreAuthProvider {
            token: Some(api_key),
        });
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(CoreAuthProvider { token: Some(token) });
    }

    if let Some(auth) = auth {
        let token = auth.get_token()?;
        Ok(CoreAuthProvider { token: Some(token) })
    } else {
        Ok(CoreAuthProvider::default())
    }
}

#[derive(Clone, Default)]
pub(crate) struct CoreAuthProvider {
    token: Option<String>,
}

impl ApiAuthProvider for CoreAuthProvider {
    fn bearer_token(&self) -> Option<String> {
        self.token.clone()
    }
}
