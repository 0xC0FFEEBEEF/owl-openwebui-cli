use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::ModelProviderInfo;
use codex_core::ThreadManager;
use codex_core::WireApi;
use codex_core::models_manager::manager::RefreshStrategy;
use codex_protocol::openai_models::ModelPreset;
use core_test_support::load_default_config_for_test;
use core_test_support::responses::listed_model;
use core_test_support::responses::mount_models_listing;
use core_test_support::responses::start_listing_server;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

/// A provider shaped like the user's self-hosted Open WebUI: a base URL owl asks for
/// models at `{base_url}/models`, with no ChatGPT-style account behind it.
fn server_provider(server: &MockServer) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "Test server".to_string(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_server_models() -> Result<()> {
    // Open WebUI serves plain model entries; owl has no model catalog of its own, so
    // each served id becomes a preset carrying owl's neutral defaults.
    let codex_home = tempdir()?;
    let config = load_default_config_for_test(&codex_home).await;
    let server = start_listing_server().await;
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "owl-alpha", "object": "model" },
                { "id": "owl-beta", "object": "model" },
            ]
        })))
        .mount(&server)
        .await;

    let manager = ThreadManager::with_models_provider(
        CodexAuth::from_api_key("sk-test"),
        server_provider(&server),
    );
    let models = manager
        .list_models(&config, RefreshStrategy::OnlineIfUncached)
        .await;

    let served: Vec<&str> = models.iter().map(|model| model.model.as_str()).collect();
    assert_eq!(served, vec!["owl-alpha", "owl-beta"]);
    // The first model the server serves is the picker default.
    assert_eq!(models.iter().filter(|model| model.is_default).count(), 1);
    assert!(models[0].is_default);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_uses_metadata_declared_by_the_server() -> Result<()> {
    // When the user's server serves full model metadata, owl shows exactly what it
    // declares, ordered by the priority the server chose.
    let codex_home = tempdir()?;
    let config = load_default_config_for_test(&codex_home).await;
    let server = start_listing_server().await;

    let mut high_priority = listed_model("server-fast");
    high_priority.priority = 1;
    let mut low_priority = listed_model("server-deep");
    low_priority.priority = 2;
    low_priority.display_name = "Deep Thinker".to_string();
    mount_models_listing(&server, vec![low_priority.clone(), high_priority.clone()]).await;

    let manager = ThreadManager::with_models_provider(
        CodexAuth::from_api_key("sk-test"),
        server_provider(&server),
    );
    let models = manager.list_models(&config, RefreshStrategy::Online).await;

    let mut expected: Vec<ModelPreset> = vec![high_priority, low_priority]
        .into_iter()
        .map(Into::into)
        .collect();
    expected[0].is_default = true;
    assert_eq!(models, expected);

    Ok(())
}
