#![allow(clippy::unwrap_used)]

use codex_core::features::Feature;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use core_test_support::load_sse_fixture_with_id;
use core_test_support::responses;
use core_test_support::responses::listed_model;
use core_test_support::responses::mount_models_listing;
use core_test_support::responses::start_listing_server;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;

const PLAIN_MODEL: &str = "plain-model";

fn sse_completed(id: &str) -> String {
    load_sse_fixture_with_id("../fixtures/completed_template.json", id)
}

#[allow(clippy::expect_used)]
fn tool_identifiers(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            tool.get("name")
                .and_then(|v| v.as_str())
                .or_else(|| tool.get("type").and_then(|v| v.as_str()))
                .map(std::string::ToString::to_string)
                .expect("tool should have either name or type")
        })
        .collect()
}

/// Tools owl exposes for a slug the server's model list says nothing about.
#[allow(clippy::expect_used)]
async fn collect_tool_identifiers_for_model(model: &str) -> Vec<String> {
    let server = start_mock_server().await;
    let sse = sse_completed(model);
    let resp_mock = responses::mount_sse_once(&server, sse).await;

    let mut builder = test_codex()
        .with_model(model)
        // Keep tool expectations stable when the default web_search mode changes.
        .with_config(|config| {
            config.web_search_mode = Some(WebSearchMode::Cached);
            config.features.enable(Feature::CollaborationModes);
        });
    let test = builder
        .build(&server)
        .await
        .expect("create test owl conversation");

    test.submit_turn("hello tools").await.expect("submit turn");

    let body = resp_mock.single_request().body_json();
    tool_identifiers(&body)
}

/// Tools owl exposes for a model the user's server declares with `model_info`.
#[allow(clippy::expect_used)]
async fn collect_tool_identifiers_for_declared_model(model: ModelInfo) -> Vec<String> {
    let slug = model.slug.clone();
    let server = start_listing_server().await;
    mount_models_listing(&server, vec![model]).await;
    let resp_mock = responses::mount_sse_once(&server, sse_completed("resp-1")).await;

    let mut builder = test_codex()
        .with_config(|config| {
            config.web_search_mode = Some(WebSearchMode::Cached);
            config.features.enable(Feature::CollaborationModes);
        })
        .with_server_listing(&slug);
    let test = builder
        .build(&server)
        .await
        .expect("create test owl conversation");

    test.submit_turn("hello tools").await.expect("submit turn");

    tool_identifiers(&resp_mock.single_request().body_json())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_selects_expected_tools() {
    skip_if_no_network!();
    use pretty_assertions::assert_eq;

    // A model owl has no metadata for gets owl's own default tool set: the plain shell
    // tool and apply_patch as an ordinary function tool, which any provider can carry.
    let default_tools = collect_tool_identifiers_for_model(PLAIN_MODEL).await;
    assert_eq!(
        default_tools,
        vec![
            "shell".to_string(),
            "list_mcp_resources".to_string(),
            "list_mcp_resource_templates".to_string(),
            "read_mcp_resource".to_string(),
            "update_plan".to_string(),
            "request_user_input".to_string(),
            "apply_patch".to_string(),
            "web_search".to_string(),
            "view_image".to_string(),
        ],
        "a model owl has no metadata for should get owl's default tool set",
    );

    // What the user's server declares is what owl exposes: this gateway asks for its
    // local shell tool.
    let mut local_shell_model = listed_model("gateway-local-shell");
    local_shell_model.shell_type = ConfigShellToolType::Local;
    let local_shell_tools = collect_tool_identifiers_for_declared_model(local_shell_model).await;
    assert_eq!(
        local_shell_tools,
        vec![
            "local_shell".to_string(),
            "list_mcp_resources".to_string(),
            "list_mcp_resource_templates".to_string(),
            "read_mcp_resource".to_string(),
            "update_plan".to_string(),
            "request_user_input".to_string(),
            "apply_patch".to_string(),
            "web_search".to_string(),
            "view_image".to_string(),
        ],
        "owl should expose the shell tool the user's server declares",
    );

    // Another gateway declares the shell_command tool instead.
    let shell_command_model = listed_model("gateway-shell-command");
    let shell_command_tools =
        collect_tool_identifiers_for_declared_model(shell_command_model).await;
    assert_eq!(
        shell_command_tools,
        vec![
            "shell_command".to_string(),
            "list_mcp_resources".to_string(),
            "list_mcp_resource_templates".to_string(),
            "read_mcp_resource".to_string(),
            "update_plan".to_string(),
            "request_user_input".to_string(),
            "apply_patch".to_string(),
            "web_search".to_string(),
            "view_image".to_string(),
        ],
        "owl should expose the shell tool the user's server declares",
    );

    // owl's own unified_exec feature swaps the shell tool for its two exec tools;
    // the slug plays no part.
    let server = start_mock_server().await;
    let resp_mock = responses::mount_sse_once(&server, sse_completed("resp-1")).await;
    let mut builder = test_codex().with_model(PLAIN_MODEL).with_config(|config| {
        config.web_search_mode = Some(WebSearchMode::Cached);
        config.features.enable(Feature::CollaborationModes);
        config.features.enable(Feature::UnifiedExec);
    });
    let test = builder
        .build(&server)
        .await
        .expect("create test owl conversation");
    test.submit_turn("hello tools").await.expect("submit turn");
    let unified_tools = tool_identifiers(&resp_mock.single_request().body_json());
    assert_eq!(
        unified_tools,
        vec![
            "exec_command".to_string(),
            "write_stdin".to_string(),
            "list_mcp_resources".to_string(),
            "list_mcp_resource_templates".to_string(),
            "read_mcp_resource".to_string(),
            "update_plan".to_string(),
            "request_user_input".to_string(),
            "apply_patch".to_string(),
            "web_search".to_string(),
            "view_image".to_string(),
        ],
        "enabling owl's unified_exec feature should swap in its exec tools",
    );
}
