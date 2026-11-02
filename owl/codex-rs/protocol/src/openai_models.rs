use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;
use strum_macros::EnumIter;
use tracing::warn;
use ts_rs::TS;

use crate::config_types::Personality;
use crate::config_types::Verbosity;

const PERSONALITY_PLACEHOLDER: &str = "{{ personality_message }}";

/// See https://platform.openai.com/docs/guides/reasoning?api-mode=responses#get-started-with-reasoning
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Display,
    JsonSchema,
    TS,
    EnumIter,
    Hash,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

/// A reasoning effort option that can be surfaced for a model.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
pub struct ReasoningEffortPreset {
    /// Effort level that the model supports.
    pub effort: ReasoningEffort,
    /// Short human description shown next to the effort in UIs.
    pub description: String,
}

/// Metadata describing a owl-supported model.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq)]
pub struct ModelPreset {
    /// Stable identifier for the preset.
    pub id: String,
    /// Model slug (e.g., "gpt-5").
    pub model: String,
    /// Display name shown in UIs.
    pub display_name: String,
    /// Short human description shown in UIs.
    pub description: String,
    /// Reasoning effort applied when none is explicitly chosen.
    pub default_reasoning_effort: ReasoningEffort,
    /// Supported reasoning effort options.
    pub supported_reasoning_efforts: Vec<ReasoningEffortPreset>,
    /// Whether this is the default model for new users.
    pub is_default: bool,
    /// Whether this preset should appear in the picker UI.
    pub show_in_picker: bool,
    /// whether this model is supported in the api
    pub supported_in_api: bool,
}

/// Visibility of a model in the picker or APIs.
#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, TS, JsonSchema, EnumIter, Display,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ModelVisibility {
    List,
    Hide,
    None,
}

/// Shell execution capability for a model.
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    TS,
    JsonSchema,
    EnumIter,
    Display,
    Hash,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ConfigShellToolType {
    Default,
    Local,
    UnifiedExec,
    Disabled,
    ShellCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchToolType {
    Freeform,
    Function,
}

/// Server-provided truncation policy metadata for a model.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TruncationMode {
    Bytes,
    Tokens,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, TS, JsonSchema)]
pub struct TruncationPolicyConfig {
    pub mode: TruncationMode,
    pub limit: i64,
}

impl TruncationPolicyConfig {
    pub const fn bytes(limit: i64) -> Self {
        Self {
            mode: TruncationMode::Bytes,
            limit,
        }
    }

    pub const fn tokens(limit: i64) -> Self {
        Self {
            mode: TruncationMode::Tokens,
            limit,
        }
    }
}

/// Semantic version triple encoded as an array in JSON (e.g. [0, 62, 0]).
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, TS, JsonSchema)]
pub struct ClientVersion(pub i32, pub i32, pub i32);

const fn default_effective_context_window_percent() -> i64 {
    95
}

/// Model metadata returned by the owl backend `/models` endpoint.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, TS, JsonSchema)]
pub struct ModelInfo {
    pub slug: String,
    pub display_name: String,
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<ReasoningEffort>,
    pub supported_reasoning_levels: Vec<ReasoningEffortPreset>,
    pub shell_type: ConfigShellToolType,
    pub visibility: ModelVisibility,
    pub supported_in_api: bool,
    pub priority: i32,
    pub base_instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_instructions_template: Option<ModelInstructionsTemplate>,
    pub supports_reasoning_summaries: bool,
    pub support_verbosity: bool,
    pub default_verbosity: Option<Verbosity>,
    pub apply_patch_tool_type: Option<ApplyPatchToolType>,
    pub truncation_policy: TruncationPolicyConfig,
    pub supports_parallel_tool_calls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// Token threshold for automatic compaction. When omitted, core derives it
    /// from `context_window` (90%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_token_limit: Option<i64>,
    /// Percentage of the context window considered usable for inputs, after
    /// reserving headroom for system prompts, tool overhead, and model output.
    #[serde(default = "default_effective_context_window_percent")]
    pub effective_context_window_percent: i64,
    pub experimental_supported_tools: Vec<String>,
}

impl ModelInfo {
    pub fn auto_compact_token_limit(&self) -> Option<i64> {
        self.auto_compact_token_limit.or_else(|| {
            self.context_window
                .map(|context_window| (context_window * 9) / 10)
        })
    }

    pub fn get_model_instructions(&self, personality: Option<Personality>) -> String {
        let Some(personality) = personality else {
            return self.base_instructions.clone();
        };
        let Some(template) = &self.model_instructions_template else {
            warn!(
                model = %self.slug,
                %personality,
                "Model personality requested but this model has no instructions template, falling back to base instructions."
            );
            return self.base_instructions.clone();
        };
        let Some(personality_message) = template
            .personality_messages
            .as_ref()
            .and_then(|personality_messages| personality_messages.0.get(&personality))
        else {
            warn!(
                model = %self.slug,
                %personality,
                "Model personality requested but this model has no message for it, falling back to base instructions."
            );
            return self.base_instructions.clone();
        };
        if template.template.is_empty() {
            // No instructions scaffold came with the model (owl's models come from
            // the user's own server), so ask for the personality on top of the
            // base instructions instead of replacing them.
            return format!(
                "{}\n\n# Personality\n\n{personality_message}",
                self.base_instructions
            );
        }
        if template.has_personality_placeholder() {
            return template
                .template
                .replace(PERSONALITY_PLACEHOLDER, personality_message.as_str());
        }
        warn!(
            model = %self.slug,
            %personality,
            "Model personality requested but model_instructions_template is invalid, falling back to base instructions."
        );
        self.base_instructions.clone()
    }
}

/// A strongly-typed template for assembling model instructions. If populated and valid, will override
/// base_instructions.
///
/// An empty `template` means the model's server did not supply an instructions
/// scaffold: the personality is then asked for on top of `base_instructions`
/// rather than replacing them, which is how owl's own personality works with any
/// model the user serves.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, TS, JsonSchema)]
pub struct ModelInstructionsTemplate {
    pub template: String,
    pub personality_messages: Option<PersonalityMessages>,
}

impl ModelInstructionsTemplate {
    fn has_personality_placeholder(&self) -> bool {
        self.template.contains(PERSONALITY_PLACEHOLDER)
    }
}

// serializes as a dictionary from personality to message
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TS, JsonSchema)]
#[serde(transparent)]
pub struct PersonalityMessages(pub BTreeMap<Personality, String>);

/// Response wrapper for `/models`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, TS, JsonSchema, Default)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
}

// convert ModelInfo to ModelPreset
impl From<ModelInfo> for ModelPreset {
    fn from(info: ModelInfo) -> Self {
        ModelPreset {
            id: info.slug.clone(),
            model: info.slug.clone(),
            display_name: info.display_name,
            description: info.description.unwrap_or_default(),
            default_reasoning_effort: info
                .default_reasoning_level
                .unwrap_or(ReasoningEffort::None),
            supported_reasoning_efforts: info.supported_reasoning_levels.clone(),
            is_default: false, // default is the highest priority available model
            show_in_picker: info.visibility == ModelVisibility::List,
            supported_in_api: info.supported_in_api,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn test_model(template: Option<ModelInstructionsTemplate>) -> ModelInfo {
        ModelInfo {
            slug: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            description: None,
            default_reasoning_level: None,
            supported_reasoning_levels: vec![],
            shell_type: ConfigShellToolType::ShellCommand,
            visibility: ModelVisibility::List,
            supported_in_api: true,
            priority: 1,
            base_instructions: "base".to_string(),
            model_instructions_template: template,
            supports_reasoning_summaries: false,
            support_verbosity: false,
            default_verbosity: None,
            apply_patch_tool_type: None,
            truncation_policy: TruncationPolicyConfig::bytes(10_000),
            supports_parallel_tool_calls: false,
            context_window: None,
            auto_compact_token_limit: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: vec![],
        }
    }

    fn personality_messages() -> PersonalityMessages {
        PersonalityMessages(BTreeMap::from([(
            Personality::Friendly,
            "friendly".to_string(),
        )]))
    }

    #[test]
    fn get_model_instructions_uses_template_when_placeholder_present() {
        let model = test_model(Some(ModelInstructionsTemplate {
            template: "Hello {{ personality_message }}".to_string(),
            personality_messages: Some(personality_messages()),
        }));

        let instructions = model.get_model_instructions(Some(Personality::Friendly));

        assert_eq!(instructions, "Hello friendly");
    }

    #[test]
    fn get_model_instructions_appends_personality_when_template_is_empty() {
        let model = test_model(Some(ModelInstructionsTemplate {
            template: String::new(),
            personality_messages: Some(personality_messages()),
        }));

        let instructions = model.get_model_instructions(Some(Personality::Friendly));

        assert_eq!(instructions, "base\n\n# Personality\n\nfriendly");
    }

    #[test]
    fn get_model_instructions_falls_back_when_placeholder_missing() {
        let model = test_model(Some(ModelInstructionsTemplate {
            template: "Hello there".to_string(),
            personality_messages: Some(personality_messages()),
        }));

        let instructions = model.get_model_instructions(Some(Personality::Friendly));

        assert_eq!(instructions, "base");
    }
}
