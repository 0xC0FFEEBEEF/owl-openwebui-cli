use codex_core::models_manager::manager::ModelsManager;
use codex_protocol::openai_models::TruncationPolicyConfig;
use core_test_support::load_default_config_for_test;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

// owl ships no model catalog: without a listing, every model gets the same neutral
// byte-based truncation default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_model_info_without_tool_output_override() {
    let codex_home = TempDir::new().expect("create temp dir");
    let config = load_default_config_for_test(&codex_home).await;

    let model_info = ModelsManager::construct_model_info_offline("server-model", &config);

    assert_eq!(
        model_info.truncation_policy,
        TruncationPolicyConfig::bytes(10_000)
    );
}

// `tool_output_token_limit` sets the size of the budget; the unit follows what the
// model's listing declared. Offline (no listing), owl's neutral default is bytes, so
// the configured token count is converted to its approximate byte size.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_model_info_with_tool_output_override() {
    let codex_home = TempDir::new().expect("create temp dir");
    let mut config = load_default_config_for_test(&codex_home).await;
    config.tool_output_token_limit = Some(123);

    let model_info = ModelsManager::construct_model_info_offline("server-model", &config);

    assert_eq!(
        model_info.truncation_policy,
        TruncationPolicyConfig::bytes(492)
    );
}
