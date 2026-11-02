#![cfg(not(target_os = "windows"))]

use std::sync::Arc;

use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::features::Feature;
use codex_core::models_manager::manager::RefreshStrategy;
use codex_core::protocol::AskForApproval;
use codex_core::protocol::EventMsg;
use codex_core::protocol::Op;
use codex_core::protocol::SandboxPolicy;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_local_shell_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use wiremock::MockServer;

const SERVER_MODEL: &str = "owl-etag-test";

/// Refresh the model list when the server reports a new model-list ETag, and do
/// not fetch the list a second time while that ETag stays the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_models_on_models_etag_mismatch_and_avoid_duplicate_models_fetch() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const ETAG_1: &str = "\"models-etag-1\"";
    const ETAG_2: &str = "\"models-etag-2\"";
    const CALL_ID: &str = "local-shell-call-1";

    let server = MockServer::start().await;

    // 1) On spawn, owl fetches the model list from the provider's own server and
    //    remembers the ETag that response came with.
    let spawn_models_mock = responses::mount_models_once_with_etag(
        &server,
        ModelsResponse {
            models: vec![server_model()],
        },
        ETAG_1,
    )
    .await;

    let auth = CodexAuth::from_api_key("dummy-test-key");
    let mut builder = test_codex().with_auth(auth).with_config(|config| {
        config.features.enable(Feature::RemoteModels);
        // Keep this test deterministic: no request retries, and a small stream retry budget.
        config.model_provider.request_max_retries = Some(0);
        config.model_provider.stream_max_retries = Some(1);
    });

    let test = builder.build(&server).await?;
    let codex = Arc::clone(&test.codex);
    let cwd = Arc::clone(&test.cwd);
    // The session runs a model the user's server actually listed.
    let session_model = test.session_configured.model.clone();
    assert_eq!(session_model, SERVER_MODEL);

    assert_eq!(spawn_models_mock.requests().len(), 1);
    assert_eq!(spawn_models_mock.single_request_path(), "/v1/models");

    // 2) When the server sends a different model-list ETag on a response, owl
    //    asks the same provider endpoint for the list again.
    let refresh_models_mock = responses::mount_models_once_with_etag(
        &server,
        ModelsResponse {
            models: vec![server_model()],
        },
        ETAG_2,
    )
    .await;

    // First request (user message) succeeds and returns a tool call. It also
    // carries a mismatched model-list ETag, which should trigger a refresh.
    let first_response_body = sse(vec![
        ev_response_created("resp-1"),
        ev_local_shell_call(CALL_ID, "completed", vec!["/bin/echo", "etag ok"]),
        ev_completed("resp-1"),
    ]);
    responses::mount_response_once(
        &server,
        sse_response(first_response_body).insert_header("X-Models-Etag", ETAG_2),
    )
    .await;

    // Second request (tool output) carries the same ETag; owl should not fetch
    // the model list again once it has already refreshed it.
    let completion_response_body = sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        ev_completed("resp-2"),
    ]);
    let tool_output_mock = responses::mount_response_once(
        &server,
        sse_response(completion_response_body).insert_header("X-Models-Etag", ETAG_2),
    )
    .await;

    codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "please run a tool".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: session_model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let _ = wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // Assert the model list was refreshed exactly once after the ETag mismatch.
    assert_eq!(refresh_models_mock.requests().len(), 1);
    assert_eq!(refresh_models_mock.single_request_path(), "/v1/models");

    // Assert the tool output request succeeded and did not trigger another fetch.
    let tool_req = tool_output_mock.single_request();
    let _ = tool_req.function_call_output(CALL_ID);
    assert_eq!(refresh_models_mock.requests().len(), 1);

    // The refreshed list is the list owl keeps using.
    let models_manager = test.thread_manager.get_models_manager();
    let models = models_manager
        .list_models(&test.config, RefreshStrategy::Offline)
        .await;
    assert!(
        models.iter().any(|preset| preset.model == SERVER_MODEL),
        "the refreshed server listing should be the model list owl uses"
    );

    Ok(())
}

fn server_model() -> ModelInfo {
    ModelInfo {
        slug: SERVER_MODEL.to_string(),
        display_name: "Server Test".to_string(),
        description: Some("a model the test server lists".to_string()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "medium".to_string(),
        }],
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority: 1,
        base_instructions: "base instructions".to_string(),
        model_instructions_template: None,
        supports_reasoning_summaries: false,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        truncation_policy: TruncationPolicyConfig::bytes(10_000),
        supports_parallel_tool_calls: false,
        context_window: Some(128_000),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
    }
}
