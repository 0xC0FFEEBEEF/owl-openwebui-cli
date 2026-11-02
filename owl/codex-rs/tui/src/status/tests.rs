use super::new_status_output;
use crate::history_cell::HistoryCell;
use codex_core::ModelProviderInfo;
use codex_core::OPENWEBUI_PROVIDER_ID;
use codex_core::WireApi;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::models_manager::manager::ModelsManager;
use codex_core::protocol::SandboxPolicy;
use codex_core::protocol::TokenUsage;
use codex_core::protocol::TokenUsageInfo;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ReasoningEffort;
use insta::assert_snapshot;
use ratatui::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

/// A server-style slug: owl's model names come from the configured provider, never
/// from a bundled catalog.
const TEST_MODEL: &str = "owl-test-model";

async fn test_config(temp_home: &TempDir) -> Config {
    let mut config = ConfigBuilder::default()
        .codex_home(temp_home.path().to_path_buf())
        .build()
        .await
        .expect("load config");
    // owl has no built-in model metadata, so the context-window row needs the config knob.
    config.model_context_window = Some(272_000);
    config
}

fn token_info_for(model_slug: &str, config: &Config, usage: &TokenUsage) -> TokenUsageInfo {
    let context_window =
        ModelsManager::construct_model_info_offline(model_slug, config).context_window;
    TokenUsageInfo {
        total_token_usage: usage.clone(),
        last_token_usage: usage.clone(),
        model_context_window: context_window,
    }
}

fn render_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn sanitize_directory(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            if let (Some(dir_pos), Some(pipe_idx)) = (line.find("Directory: "), line.rfind('│')) {
                let prefix = &line[..dir_pos + "Directory: ".len()];
                let suffix = &line[pipe_idx..];
                let content_width = pipe_idx.saturating_sub(dir_pos + "Directory: ".len());
                let replacement = "[[workspace]]";
                let mut rebuilt = prefix.to_string();
                rebuilt.push_str(replacement);
                if content_width > replacement.len() {
                    rebuilt.push_str(&" ".repeat(content_width - replacement.len()));
                }
                rebuilt.push_str(suffix);
                rebuilt
            } else {
                line
            }
        })
        .collect()
}

/// The rows owl used to copy from the ChatGPT account page. None of them may come back.
const FORBIDDEN_STATUS_ROWS: [&str; 8] = [
    "chatgpt.com",
    "Account:",
    "Credits",
    "5h limit",
    "Weekly limit",
    "Monthly limit",
    "plan",
    "usage limit",
];

fn assert_no_account_rows(rendered: &[String]) {
    let joined = rendered.join("\n");
    for needle in FORBIDDEN_STATUS_ROWS {
        assert!(
            !joined.contains(needle),
            "the /status card must not mention {needle:?} (owl talks to the user's own \
             server, which has no account plan, credits or rate-limit windows):\n{joined}"
        );
    }
}

#[tokio::test]
async fn status_snapshot_shows_the_provider_and_reasoning_effort() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some(TEST_MODEL.to_string());
    config.model_reasoning_summary = ReasoningSummary::Detailed;
    config.model_reasoning_effort = Some(ReasoningEffort::High);
    config
        .sandbox_policy
        .set(SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        })
        .expect("set sandbox policy");

    config.cwd = PathBuf::from("/workspace/tests");

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 150,
        total_tokens: 2_250,
    };

    let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);

    let reasoning_effort_override = Some(Some(ReasoningEffort::High));
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        None,
        &model_slug,
        None,
        reasoning_effort_override,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    assert_no_account_rows(&rendered_lines);
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_snapshot_includes_forked_from() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some(TEST_MODEL.to_string());
    config.cwd = PathBuf::from("/workspace/tests");

    let usage = TokenUsage {
        input_tokens: 800,
        cached_input_tokens: 0,
        output_tokens: 400,
        reasoning_output_tokens: 0,
        total_tokens: 1_200,
    };

    let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let session_id =
        ThreadId::from_string("0f0f3c13-6cf9-4aa4-8b80-7d49c2f1be2e").expect("session id");
    let forked_from =
        ThreadId::from_string("e9f18a88-8081-4e51-9d4e-8af5cde2d8dd").expect("forked id");

    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &Some(session_id),
        Some(forked_from),
        &model_slug,
        None,
        None,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    assert_no_account_rows(&rendered_lines);
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_card_token_usage_excludes_cached_tokens() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some(TEST_MODEL.to_string());
    config.cwd = PathBuf::from("/workspace/tests");

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 0,
        total_tokens: 2_100,
    };

    let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        None,
        &model_slug,
        None,
        None,
    );
    let rendered = render_lines(&composite.display_lines(120));

    assert!(
        rendered.iter().all(|line| !line.contains("cached")),
        "cached tokens should not be displayed, got: {rendered:?}"
    );
}

#[tokio::test]
async fn status_snapshot_truncates_in_narrow_terminal() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some(TEST_MODEL.to_string());
    config.model_reasoning_summary = ReasoningSummary::Detailed;
    config.cwd = PathBuf::from("/workspace/tests");

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 150,
        total_tokens: 2_250,
    };

    let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let reasoning_effort_override = Some(Some(ReasoningEffort::High));
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        None,
        &model_slug,
        None,
        reasoning_effort_override,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(70));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    assert_no_account_rows(&rendered_lines);
    let sanitized = sanitize_directory(rendered_lines).join("\n");

    assert_snapshot!(sanitized);
}

/// The user's rule: owl is for their own models only, so `/status` never renders the
/// account, rate-limit window or credit rows, whatever provider is configured.
#[tokio::test]
async fn status_card_has_no_account_limits_or_credits_for_any_provider() {
    for provider in [built_in_openwebui_provider(), custom_responses_provider()] {
        let temp_home = TempDir::new().expect("temp home");
        let mut config = test_config(&temp_home).await;
        let provider_id = provider.0;
        let provider_name = provider.1;
        if provider_id != OPENWEBUI_PROVIDER_ID {
            let info = provider_info(provider_id, provider_name);
            config
                .model_providers
                .insert(provider_id.to_string(), info.clone());
            config.model_provider = info;
            config.model_provider_id = provider_id.to_string();
        }
        config.model = Some(TEST_MODEL.to_string());
        config.cwd = PathBuf::from("/workspace/tests");

        let usage = TokenUsage {
            input_tokens: 500,
            cached_input_tokens: 0,
            output_tokens: 250,
            reasoning_output_tokens: 0,
            total_tokens: 750,
        };

        let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
        let token_info = token_info_for(&model_slug, &config, &usage);
        let composite = new_status_output(
            &config,
            Some(&token_info),
            &usage,
            &None,
            None,
            &model_slug,
            None,
            None,
        );
        let rendered = render_lines(&composite.display_lines(80));

        assert_no_account_rows(&rendered);
        assert!(
            rendered.iter().any(|line| line.contains(provider_name)),
            "the card should name the provider the session is talking to ({provider_name}):\n{}",
            rendered.join("\n")
        );
    }
}

fn built_in_openwebui_provider() -> (&'static str, &'static str) {
    (OPENWEBUI_PROVIDER_ID, "Open WebUI")
}

fn custom_responses_provider() -> (&'static str, &'static str) {
    ("local-gateway", "Local Gateway")
}

fn provider_info(id: &str, name: &str) -> ModelProviderInfo {
    ModelProviderInfo {
        name: name.to_string(),
        base_url: Some(format!("http://127.0.0.1:4444/{id}/v1")),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
    }
}

#[tokio::test]
async fn status_context_window_uses_last_usage() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model_context_window = Some(272_000);

    let total_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 102_000,
    };
    let last_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 13_679,
    };

    let model_slug = ModelsManager::get_model_offline(config.model.as_deref());
    let token_info = TokenUsageInfo {
        total_token_usage: total_usage.clone(),
        last_token_usage: last_usage,
        model_context_window: config.model_context_window,
    };
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &total_usage,
        &None,
        None,
        &model_slug,
        None,
        None,
    );
    let rendered_lines = render_lines(&composite.display_lines(80));
    let context_line = rendered_lines
        .into_iter()
        .find(|line| line.contains("Context window"))
        .expect("context line");

    assert!(
        context_line.contains("13.7K used / 272K"),
        "expected context line to reflect last usage tokens, got: {context_line}"
    );
    assert!(
        !context_line.contains("102K"),
        "context line should not use total aggregated tokens, got: {context_line}"
    );
}
