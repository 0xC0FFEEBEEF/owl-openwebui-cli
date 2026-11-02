use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::mount_openwebui_models_listing;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_protocol::openai_models::ReasoningEffort;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

/// The models a test server claims to serve, in the shape Open WebUI's
/// `/api/v1/models` returns them. owl lists exactly these: it ships no model list
/// of its own.
fn served_models() -> Vec<Value> {
    vec![
        json!({"id": "mock-gpt-mini", "object": "model", "name": "Mock GPT Mini"}),
        json!({"id": "mock-second", "object": "model", "name": "Mock Second"}),
        json!({"id": "mock-third", "object": "model", "name": "Mock Third"}),
        json!({"id": "mock-fourth", "object": "model", "name": "Mock Fourth"}),
    ]
}

/// What owl reports for a model the user's server listed. The server's listing
/// carries no reasoning-effort metadata, so none is offered; the first model is
/// the default.
fn listed_model(id: &str, display_name: &str, is_default: bool) -> Model {
    Model {
        id: id.to_string(),
        model: id.to_string(),
        display_name: display_name.to_string(),
        description: String::new(),
        supported_reasoning_efforts: Vec::new(),
        default_reasoning_effort: ReasoningEffort::None,
        is_default,
    }
}

#[tokio::test]
async fn list_models_returns_all_models_with_large_limit() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = responses::start_listing_server().await;
    let base_url = mount_openwebui_models_listing(&server, served_models()).await;
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[("OPENWEBUI_BASE_URL", Some(base_url.as_str()))],
    )
    .await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    let expected_models = vec![
        listed_model("mock-gpt-mini", "Mock GPT Mini", true),
        listed_model("mock-second", "Mock Second", false),
        listed_model("mock-third", "Mock Third", false),
        listed_model("mock-fourth", "Mock Fourth", false),
    ];

    assert_eq!(items, expected_models);
    assert!(next_cursor.is_none());

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        1,
        "expected a single GET to the server's model listing"
    );

    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = responses::start_listing_server().await;
    let base_url = mount_openwebui_models_listing(&server, served_models()).await;
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[("OPENWEBUI_BASE_URL", Some(base_url.as_str()))],
    )
    .await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let mut cursor: Option<String> = None;
    let mut seen: Vec<String> = Vec::new();

    for page in 0..served_models().len() {
        let request_id = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: cursor.clone(),
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;

        let ModelListResponse {
            data: items,
            next_cursor,
        } = to_response::<ModelListResponse>(response)?;

        assert_eq!(items.len(), 1, "page {page} should hold one model");
        seen.push(items[0].id.clone());
        cursor = next_cursor;
    }

    assert!(
        cursor.is_none(),
        "the last page should not offer another cursor"
    );
    assert_eq!(
        seen,
        vec![
            "mock-gpt-mini".to_string(),
            "mock-second".to_string(),
            "mock-third".to_string(),
            "mock-fourth".to_string(),
        ],
    );
    Ok(())
}

#[tokio::test]
async fn list_models_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = responses::start_listing_server().await;
    let base_url = mount_openwebui_models_listing(&server, served_models()).await;
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[("OPENWEBUI_BASE_URL", Some(base_url.as_str()))],
    )
    .await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: Some("invalid".to_string()),
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "invalid cursor: invalid");
    Ok(())
}

#[tokio::test]
async fn list_models_reports_no_models_when_the_server_lists_none() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = responses::start_listing_server().await;
    let base_url = mount_openwebui_models_listing(&server, Vec::new()).await;
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[("OPENWEBUI_BASE_URL", Some(base_url.as_str()))],
    )
    .await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    assert!(
        items.is_empty(),
        "a server that lists no models leaves owl with nothing to pick: {items:?}"
    );
    assert_eq!(next_cursor, None);
    Ok(())
}
