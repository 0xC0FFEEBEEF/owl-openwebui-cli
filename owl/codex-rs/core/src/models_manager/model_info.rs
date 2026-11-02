use std::collections::BTreeMap;

use codex_protocol::config_types::Personality;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelInstructionsTemplate;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::PersonalityMessages;
use codex_protocol::openai_models::TruncationMode;
use codex_protocol::openai_models::TruncationPolicyConfig;

use crate::config::Config;
use crate::truncate::approx_bytes_for_tokens;

pub const BASE_INSTRUCTIONS: &str = include_str!("../../prompt.md");

const PERSONALITY_FRIENDLY: &str = include_str!("../../templates/personalities/friendly.md");
const PERSONALITY_PRAGMATIC: &str = include_str!("../../templates/personalities/pragmatic.md");

macro_rules! model_info {
    (
        $slug:expr $(, $key:ident : $value:expr )* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut model = ModelInfo {
            slug: $slug.to_string(),
            display_name: $slug.to_string(),
            description: None,
            // This is primarily used when remote metadata is available. When running
            // offline, core generally omits the effort field unless explicitly
            // configured by the user.
            default_reasoning_level: None,
            supported_reasoning_levels: Vec::new(),
            shell_type: ConfigShellToolType::Default,
            visibility: ModelVisibility::None,
            supported_in_api: true,
            priority: 99,
            base_instructions: BASE_INSTRUCTIONS.to_string(),
            model_instructions_template: Some(personality_template()),
            supports_reasoning_summaries: false,
            support_verbosity: false,
            default_verbosity: None,
            // owl ships no model catalog, so the patch tool is offered in the form
            // every provider can carry: a chat-completions server only accepts plain
            // function tools, and owl drops anything else there. The freeform form
            // stays available through `apply_patch_freeform = true` and through a
            // gateway listing that declares it (both on the responses wire).
            apply_patch_tool_type: Some(ApplyPatchToolType::Function),
            truncation_policy: TruncationPolicyConfig::bytes(10_000),
            supports_parallel_tool_calls: false,
            context_window: None,
            auto_compact_token_limit: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: Vec::new(),
        };

        $(
            model.$key = $value;
        )*
        model
    }};
}

pub(crate) fn with_config_overrides(mut model: ModelInfo, config: &Config) -> ModelInfo {
    if let Some(supports_reasoning_summaries) = config.model_supports_reasoning_summaries {
        model.supports_reasoning_summaries = supports_reasoning_summaries;
    }
    if let Some(context_window) = config.model_context_window {
        model.context_window = Some(context_window);
    }
    if let Some(auto_compact_token_limit) = config.model_auto_compact_token_limit {
        model.auto_compact_token_limit = Some(auto_compact_token_limit);
    }
    if let Some(token_limit) = config.tool_output_token_limit {
        model.truncation_policy = match model.truncation_policy.mode {
            TruncationMode::Bytes => {
                let byte_limit =
                    i64::try_from(approx_bytes_for_tokens(token_limit)).unwrap_or(i64::MAX);
                TruncationPolicyConfig::bytes(byte_limit)
            }
            TruncationMode::Tokens => {
                let limit = i64::try_from(token_limit).unwrap_or(i64::MAX);
                TruncationPolicyConfig::tokens(limit)
            }
        };
    }

    if let Some(base_instructions) = &config.base_instructions {
        model.base_instructions = base_instructions.clone();
        model.model_instructions_template = None;
    }
    model
}

/// owl's personality instructions, offered to every model the user's server serves.
///
/// `template` is empty on purpose: it asks for the personality on top of the base
/// instructions instead of replacing them with a model-specific prompt owl does not have.
fn personality_template() -> ModelInstructionsTemplate {
    ModelInstructionsTemplate {
        template: String::new(),
        personality_messages: Some(PersonalityMessages(BTreeMap::from([
            (Personality::Friendly, PERSONALITY_FRIENDLY.to_string()),
            (Personality::Pragmatic, PERSONALITY_PRAGMATIC.to_string()),
        ]))),
    }
}

/// Look up model metadata for `slug`.
///
/// owl ships no model catalog: every model is one the user's own server serves, so
/// the defaults here are deliberately neutral. Anything owl cannot know is settable
/// in config.toml: `model_context_window`, `model_auto_compact_token_limit`,
/// `tool_output_token_limit`, `model_supports_reasoning_summaries`,
/// `base_instructions`, and `model_reasoning_effort`.
pub(crate) fn find_model_info_for_slug(slug: &str) -> ModelInfo {
    model_info!(slug)
}
