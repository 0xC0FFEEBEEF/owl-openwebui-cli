use crate::auth::AuthCredentialsStoreMode;
use crate::config::edit::ConfigEdit;
use crate::config::edit::ConfigEditsBuilder;
use crate::config::types::DEFAULT_OTEL_ENVIRONMENT;
use crate::config::types::DEFAULT_WEB_SEARCH_MAX_RESULTS;
use crate::config::types::DEFAULT_WEB_SEARCH_TIMEOUT;
use crate::config::types::History;
use crate::config::types::McpServerConfig;
use crate::config::types::McpServerDisabledReason;
use crate::config::types::McpServerTransportConfig;
use crate::config::types::Notice;
use crate::config::types::Notifications;
use crate::config::types::OtelConfig;
use crate::config::types::OtelConfigToml;
use crate::config::types::OtelExporterKind;
use crate::config::types::SandboxWorkspaceWrite;
use crate::config::types::ShellEnvironmentPolicy;
use crate::config::types::ShellEnvironmentPolicyToml;
use crate::config::types::SkillsConfig;
use crate::config::types::Tui;
use crate::config::types::UriBasedFileOpener;
use crate::config::types::WebSearchBackend;
use crate::config_loader::ConfigLayerStack;
use crate::config_loader::ConfigRequirements;
use crate::config_loader::LoaderOverrides;
use crate::config_loader::McpServerIdentity;
use crate::config_loader::McpServerRequirement;
use crate::config_loader::Sourced;
use crate::config_loader::load_config_layers_state;
use crate::features::Feature;
use crate::features::FeatureOverrides;
use crate::features::Features;
use crate::features::FeaturesToml;
use crate::git_info::resolve_root_git_project_for_trust;
use crate::model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::OLLAMA_CHAT_PROVIDER_ID;
use crate::model_provider_info::OLLAMA_OSS_PROVIDER_ID;
use crate::model_provider_info::OPENWEBUI_PROVIDER_ID;
use crate::model_provider_info::built_in_model_providers;
use crate::project_doc::DEFAULT_PROJECT_DOC_FILENAME;
use crate::project_doc::LOCAL_PROJECT_DOC_FILENAME;
use crate::protocol::AskForApproval;
use crate::protocol::SandboxPolicy;
use codex_app_server_protocol::Tools;
use codex_app_server_protocol::UserSavedConfig;
use codex_protocol::config_types::AltScreenMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::TrustLevel;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rmcp_client::OAuthCredentialsStoreMode;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use dirs::home_dir;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use similar::DiffableStr;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
#[cfg(test)]
use tempfile::tempdir;

use crate::config::profile::ConfigProfile;
use toml::Value as TomlValue;
use toml_edit::DocumentMut;

mod constraint;
pub mod edit;
pub mod profile;
pub mod schema;
pub mod service;
pub mod types;
pub use constraint::Constrained;
pub use constraint::ConstraintError;
pub use constraint::ConstraintResult;

pub use service::ConfigService;
pub use service::ConfigServiceError;

pub use codex_git::GhostSnapshotConfig;

/// Maximum number of bytes of the documentation that will be embedded. Larger
/// files are *silently truncated* to this size so we do not take up too much of
/// the context window.
pub(crate) const PROJECT_DOC_MAX_BYTES: usize = 32 * 1024; // 32 KiB
pub(crate) const DEFAULT_AGENT_MAX_THREADS: Option<usize> = Some(12);

pub const CONFIG_TOML_FILE: &str = "config.toml";

/// Environment variable that points owl at its own settings home.
pub const OWL_HOME_ENV_VAR: &str = "OWL_HOME";

/// Settings home owl uses when `OWL_HOME` is not set, relative to the user's home.
pub const OWL_HOME_DIR_NAME: &str = ".owl";

#[cfg(test)]
pub(crate) fn test_config() -> Config {
    let codex_home = tempdir().expect("create temp dir");
    Config::load_from_base_config_with_overrides(
        ConfigToml::default(),
        ConfigOverrides::default(),
        codex_home.path().to_path_buf(),
    )
    .expect("load default test config")
}

/// Application configuration loaded from disk and merged with overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Provenance for how this [`Config`] was derived (merged layers + enforced
    /// requirements).
    pub config_layer_stack: ConfigLayerStack,

    /// Optional override of model selection.
    pub model: Option<String>,

    /// Model used specifically for review sessions.
    pub review_model: Option<String>,

    /// Size of the context window for the model, in tokens.
    pub model_context_window: Option<i64>,

    /// Token usage threshold triggering auto-compaction of conversation history.
    pub model_auto_compact_token_limit: Option<i64>,

    /// Key into the model_providers map that specifies which provider to use.
    pub model_provider_id: String,

    /// Info needed to make an API request to the model.
    pub model_provider: ModelProviderInfo,

    /// Optionally specify the personality of the model
    pub model_personality: Option<Personality>,

    /// Approval policy for executing commands.
    pub approval_policy: Constrained<AskForApproval>,

    pub sandbox_policy: Constrained<SandboxPolicy>,

    /// True if the user passed in an override or set a value in config.toml
    /// for either of approval_policy or sandbox_mode.
    pub did_user_set_custom_approval_policy_or_sandbox_mode: bool,

    /// On Windows, indicates that a previously configured workspace-write sandbox
    /// was coerced to read-only because native auto mode is unsupported.
    pub forced_auto_mode_downgraded_on_windows: bool,

    pub shell_environment_policy: ShellEnvironmentPolicy,

    /// When `true`, `AgentReasoning` events emitted by the backend will be
    /// suppressed from the frontend output. This can reduce visual noise when
    /// users are only interested in the final agent responses.
    pub hide_agent_reasoning: bool,

    /// When set to `true`, `AgentReasoningRawContentEvent` events will be shown in the UI/output.
    /// Defaults to `false`.
    pub show_raw_agent_reasoning: bool,

    /// User-provided instructions from AGENTS.md.
    pub user_instructions: Option<String>,

    /// Base instructions override.
    pub base_instructions: Option<String>,

    /// Developer instructions override injected as a separate message.
    pub developer_instructions: Option<String>,

    /// Compact prompt override.
    pub compact_prompt: Option<String>,

    /// Optional external notifier command. When set, owl will spawn this
    /// program after each completed *turn* (i.e. when the agent finishes
    /// processing a user submission). The value must be the full command
    /// broken into argv tokens **without** the trailing JSON argument - owl
    /// appends one extra argument containing a JSON payload describing the
    /// event.
    ///
    /// Example `~/.owl/config.toml` snippet:
    ///
    /// ```toml
    /// notify = ["notify-send", "owl"]
    /// ```
    ///
    /// which will be invoked as:
    ///
    /// ```shell
    /// notify-send owl '{"type":"agent-turn-complete","turn-id":"12345"}'
    /// ```
    ///
    /// If unset the feature is disabled.
    pub notify: Option<Vec<String>>,

    /// TUI notifications preference. When set, the TUI will send OSC 9 notifications on approvals
    /// and turn completions when not focused.
    pub tui_notifications: Notifications,

    /// Enable ASCII animations and shimmer effects in the TUI.
    pub animations: bool,

    /// Show startup tooltips in the TUI welcome screen.
    pub show_tooltips: bool,

    /// Start the TUI in the specified collaboration mode (plan/execute/etc.).
    pub experimental_mode: Option<ModeKind>,

    /// Controls whether the TUI uses the terminal's alternate screen buffer.
    ///
    /// This is the same `tui.alternate_screen` value from `config.toml` (see [`Tui`]).
    /// - `auto` (default): Disable alternate screen in Zellij, enable elsewhere.
    /// - `always`: Always use alternate screen (original behavior).
    /// - `never`: Never use alternate screen (inline mode, preserves scrollback).
    pub tui_alternate_screen: AltScreenMode,

    /// The directory that should be treated as the current working directory
    /// for the session. All relative paths inside the business-logic layer are
    /// resolved against this path.
    pub cwd: PathBuf,

    /// Preferred store for CLI auth credentials.
    /// file (default): Use a file in the owl home directory.
    /// keyring: Use an OS-specific keyring service.
    /// auto: Use the OS-specific keyring service if available, otherwise use a file.
    pub cli_auth_credentials_store_mode: AuthCredentialsStoreMode,

    /// Definition for MCP servers that owl can reach out to for tool calls.
    pub mcp_servers: Constrained<HashMap<String, McpServerConfig>>,

    /// Preferred store for MCP OAuth credentials.
    /// keyring: Use an OS-specific keyring service.
    ///          Credentials stored in the keyring will only be readable by owl unless the user explicitly grants access via OS-level keyring access.
    ///          https://github.com/openai/codex/blob/main/codex-rs/rmcp-client/src/oauth.rs#L2
    /// file: CODEX_HOME/.credentials.json
    ///       This file will be readable to owl and other applications running as the same user.
    /// auto (default): keyring if available, otherwise file.
    pub mcp_oauth_credentials_store_mode: OAuthCredentialsStoreMode,

    /// Optional fixed port to use for the local HTTP callback server used during MCP OAuth login.
    ///
    /// When unset, owl will bind to an ephemeral port chosen by the OS.
    pub mcp_oauth_callback_port: Option<u16>,

    /// Combined provider map (defaults merged with user-defined overrides).
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Maximum number of bytes to include from an AGENTS.md project doc file.
    pub project_doc_max_bytes: usize,

    /// Additional filenames to try when looking for project-level docs.
    pub project_doc_fallback_filenames: Vec<String>,

    /// Token budget applied when storing tool/function outputs in the context manager.
    pub tool_output_token_limit: Option<usize>,

    /// Maximum number of agent threads that can be open concurrently.
    pub agent_max_threads: Option<usize>,

    /// Directory containing all owl state (defaults to `~/.owl` but can be
    /// overridden by the `OWL_HOME` (or legacy `CODEX_HOME`) environment
    /// variable).
    pub codex_home: PathBuf,

    /// Settings that govern if and what will be written to `~/.owl/history.jsonl`.
    pub history: History,

    /// When true, session is not persisted on disk. Default to `false`
    pub ephemeral: bool,

    /// Optional URI-based file opener. If set, citations to files in the model
    /// output will be hyperlinked using the specified URI scheme.
    pub file_opener: UriBasedFileOpener,

    /// Path to the `codex-linux-sandbox` executable. This must be set if
    /// [`crate::exec::SandboxType::LinuxSeccomp`] is used. Note that this
    /// cannot be set in the config file: it must be set in code via
    /// [`ConfigOverrides`].
    ///
    /// When this program is invoked, arg0 will be set to `codex-linux-sandbox`.
    pub codex_linux_sandbox_exe: Option<PathBuf>,

    /// Value to use for `reasoning.effort` when making a request using the
    /// Responses API.
    pub model_reasoning_effort: Option<ReasoningEffort>,

    /// If not "none", the value to use for `reasoning.summary` when making a
    /// request using the Responses API.
    pub model_reasoning_summary: ReasoningSummary,

    /// Optional override to force-enable reasoning summaries for the configured model.
    pub model_supports_reasoning_summaries: Option<bool>,

    /// Optional verbosity control for GPT-5 models (Responses API `text.verbosity`).
    pub model_verbosity: Option<Verbosity>,

    /// Include the `apply_patch` tool for models that benefit from invoking
    /// file edits as a structured tool call. When unset, this falls back to the
    /// model info's default preference.
    pub include_apply_patch_tool: bool,

    /// Explicit or feature-derived web search mode.
    pub web_search_mode: Option<WebSearchMode>,

    /// Resolved endpoint for the locally executed `web_search` tool, present
    /// when the `[search]` table configures one.
    pub web_search_backend: Option<WebSearchBackend>,

    /// If set to `true`, used only the experimental unified exec tool.
    pub use_experimental_unified_exec_tool: bool,

    /// Settings for ghost snapshots (used for undo).
    pub ghost_snapshot: GhostSnapshotConfig,

    /// Centralized feature flags; source of truth for feature gating.
    pub features: Features,

    /// The active profile name used to derive this `Config` (if any).
    pub active_profile: Option<String>,

    /// The currently active project config, resolved by checking if cwd:
    /// is (1) part of a git repo, (2) a git worktree, or (3) just using the cwd
    pub active_project: ProjectConfig,

    /// Tracks whether the Windows onboarding screen has been acknowledged.
    pub windows_wsl_setup_acknowledged: bool,

    /// Collection of various notices we show the user
    pub notices: Notice,

    /// When true, disables burst-paste detection for typed input entirely.
    /// All characters are inserted as they are received, and no buffering
    /// or placeholder replacement will occur for fast keypress bursts.
    pub disable_paste_burst: bool,

    /// When `false`, disables analytics across owl product surfaces in this machine.
    /// Voluntarily left as Optional because the default value might depend on the client.
    pub analytics_enabled: Option<bool>,

    /// When `false`, disables feedback collection across owl product surfaces.
    /// Defaults to `true`.
    pub feedback_enabled: bool,

    /// OTEL configuration (exporter type, endpoint, headers, etc.).
    pub otel: crate::config::types::OtelConfig,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    codex_home: Option<PathBuf>,
    cli_overrides: Option<Vec<(String, TomlValue)>>,
    harness_overrides: Option<ConfigOverrides>,
    loader_overrides: Option<LoaderOverrides>,
    fallback_cwd: Option<PathBuf>,
}

impl ConfigBuilder {
    pub fn codex_home(mut self, codex_home: PathBuf) -> Self {
        self.codex_home = Some(codex_home);
        self
    }

    pub fn cli_overrides(mut self, cli_overrides: Vec<(String, TomlValue)>) -> Self {
        self.cli_overrides = Some(cli_overrides);
        self
    }

    pub fn harness_overrides(mut self, harness_overrides: ConfigOverrides) -> Self {
        self.harness_overrides = Some(harness_overrides);
        self
    }

    pub fn loader_overrides(mut self, loader_overrides: LoaderOverrides) -> Self {
        self.loader_overrides = Some(loader_overrides);
        self
    }

    pub fn fallback_cwd(mut self, fallback_cwd: Option<PathBuf>) -> Self {
        self.fallback_cwd = fallback_cwd;
        self
    }

    pub async fn build(self) -> std::io::Result<Config> {
        let Self {
            codex_home,
            cli_overrides,
            harness_overrides,
            loader_overrides,
            fallback_cwd,
        } = self;
        let codex_home = codex_home.map_or_else(find_codex_home, std::io::Result::Ok)?;
        let cli_overrides = cli_overrides.unwrap_or_default();
        let mut harness_overrides = harness_overrides.unwrap_or_default();
        let loader_overrides = loader_overrides.unwrap_or_default();
        let cwd_override = harness_overrides.cwd.as_deref().or(fallback_cwd.as_deref());
        let cwd = match cwd_override {
            Some(path) => AbsolutePathBuf::try_from(path)?,
            None => AbsolutePathBuf::current_dir()?,
        };
        harness_overrides.cwd = Some(cwd.to_path_buf());
        let config_layer_stack =
            load_config_layers_state(&codex_home, Some(cwd), &cli_overrides, loader_overrides)
                .await?;
        let merged_toml = config_layer_stack.effective_config();

        // Note that each layer in ConfigLayerStack should have resolved
        // relative paths to absolute paths based on the parent folder of the
        // respective config file, so we should be safe to deserialize without
        // AbsolutePathBufGuard here.
        let config_toml: ConfigToml = match merged_toml.try_into() {
            Ok(config_toml) => config_toml,
            Err(err) => {
                if let Some(config_error) =
                    crate::config_loader::first_layer_config_error(&config_layer_stack).await
                {
                    return Err(crate::config_loader::io_error_from_config_error(
                        std::io::ErrorKind::InvalidData,
                        config_error,
                        Some(err),
                    ));
                }
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, err));
            }
        };
        Config::load_config_with_layer_stack(
            config_toml,
            harness_overrides,
            codex_home,
            config_layer_stack,
        )
    }
}

impl Config {
    /// This is the preferred way to create an instance of [Config].
    pub async fn load_with_cli_overrides(
        cli_overrides: Vec<(String, TomlValue)>,
    ) -> std::io::Result<Self> {
        ConfigBuilder::default()
            .cli_overrides(cli_overrides)
            .build()
            .await
    }

    /// Load a default configuration when user config files are invalid.
    pub fn load_default_with_cli_overrides(
        cli_overrides: Vec<(String, TomlValue)>,
    ) -> std::io::Result<Self> {
        let codex_home = find_codex_home()?;
        let mut merged = toml::Value::try_from(ConfigToml::default()).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to serialize default config: {e}"),
            )
        })?;
        let cli_layer = crate::config_loader::build_cli_overrides_layer(&cli_overrides);
        crate::config_loader::merge_toml_values(&mut merged, &cli_layer);
        let config_toml = deserialize_config_toml_with_base(merged, &codex_home)?;
        Self::load_config_with_layer_stack(
            config_toml,
            ConfigOverrides::default(),
            codex_home,
            ConfigLayerStack::default(),
        )
    }

    /// This is a secondary way of creating [Config], which is appropriate when
    /// the harness is meant to be used with a specific configuration that
    /// ignores user settings. For example, the `codex exec` subcommand is
    /// designed to use [AskForApproval::Never] exclusively.
    ///
    /// Further, [ConfigOverrides] contains some options that are not supported
    /// in [ConfigToml], such as `cwd` and `codex_linux_sandbox_exe`.
    pub async fn load_with_cli_overrides_and_harness_overrides(
        cli_overrides: Vec<(String, TomlValue)>,
        harness_overrides: ConfigOverrides,
    ) -> std::io::Result<Self> {
        ConfigBuilder::default()
            .cli_overrides(cli_overrides)
            .harness_overrides(harness_overrides)
            .build()
            .await
    }
}

/// DEPRECATED: Use [Config::load_with_cli_overrides()] instead because working
/// with [ConfigToml] directly means that [ConfigRequirements] have not been
/// applied yet, which risks failing to enforce required constraints.
pub async fn load_config_as_toml_with_cli_overrides(
    codex_home: &Path,
    cwd: &AbsolutePathBuf,
    cli_overrides: Vec<(String, TomlValue)>,
) -> std::io::Result<ConfigToml> {
    let config_layer_stack = load_config_layers_state(
        codex_home,
        Some(cwd.clone()),
        &cli_overrides,
        LoaderOverrides::default(),
    )
    .await?;

    let merged_toml = config_layer_stack.effective_config();
    let cfg = deserialize_config_toml_with_base(merged_toml, codex_home).map_err(|e| {
        tracing::error!("Failed to deserialize overridden config: {e}");
        e
    })?;

    Ok(cfg)
}

pub(crate) fn deserialize_config_toml_with_base(
    root_value: TomlValue,
    config_base_dir: &Path,
) -> std::io::Result<ConfigToml> {
    // This guard ensures that any relative paths that is deserialized into an
    // [AbsolutePathBuf] is resolved against `config_base_dir`.
    let _guard = AbsolutePathBufGuard::new(config_base_dir);
    root_value
        .try_into()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn filter_mcp_servers_by_requirements(
    mcp_servers: &mut HashMap<String, McpServerConfig>,
    mcp_requirements: Option<&Sourced<BTreeMap<String, McpServerRequirement>>>,
) {
    let Some(allowlist) = mcp_requirements else {
        return;
    };

    let source = allowlist.source.clone();
    for (name, server) in mcp_servers.iter_mut() {
        let allowed = allowlist
            .value
            .get(name)
            .is_some_and(|requirement| mcp_server_matches_requirement(requirement, server));
        if allowed {
            server.disabled_reason = None;
        } else {
            server.enabled = false;
            server.disabled_reason = Some(McpServerDisabledReason::Requirements {
                source: source.clone(),
            });
        }
    }
}

fn constrain_mcp_servers(
    mcp_servers: HashMap<String, McpServerConfig>,
    mcp_requirements: Option<&Sourced<BTreeMap<String, McpServerRequirement>>>,
) -> ConstraintResult<Constrained<HashMap<String, McpServerConfig>>> {
    if mcp_requirements.is_none() {
        return Ok(Constrained::allow_any(mcp_servers));
    }

    let mcp_requirements = mcp_requirements.cloned();
    Constrained::normalized(mcp_servers, move |mut servers| {
        filter_mcp_servers_by_requirements(&mut servers, mcp_requirements.as_ref());
        servers
    })
}

fn mcp_server_matches_requirement(
    requirement: &McpServerRequirement,
    server: &McpServerConfig,
) -> bool {
    match &requirement.identity {
        McpServerIdentity::Command {
            command: want_command,
        } => matches!(
            &server.transport,
            McpServerTransportConfig::Stdio { command: got_command, .. }
                if got_command == want_command
        ),
        McpServerIdentity::Url { url: want_url } => matches!(
            &server.transport,
            McpServerTransportConfig::StreamableHttp { url: got_url, .. }
                if got_url == want_url
        ),
    }
}

pub async fn load_global_mcp_servers(
    codex_home: &Path,
) -> std::io::Result<BTreeMap<String, McpServerConfig>> {
    // In general, Config::load_with_cli_overrides() should be used to load the
    // full config with requirements.toml applied, but in this case, we need
    // access to the raw TOML in order to warn the user about deprecated fields.
    //
    // Note that a more precise way to do this would be to audit the individual
    // config layers for deprecated fields rather than reporting on the merged
    // result.
    let cli_overrides = Vec::<(String, TomlValue)>::new();
    // There is no cwd/project context for this query, so this will not include
    // MCP servers defined in in-repo .owl/ folders.
    let cwd: Option<AbsolutePathBuf> = None;
    let config_layer_stack =
        load_config_layers_state(codex_home, cwd, &cli_overrides, LoaderOverrides::default())
            .await?;
    let merged_toml = config_layer_stack.effective_config();
    let Some(servers_value) = merged_toml.get("mcp_servers") else {
        return Ok(BTreeMap::new());
    };

    ensure_no_inline_bearer_tokens(servers_value)?;

    servers_value
        .clone()
        .try_into()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// We briefly allowed plain text bearer_token fields in MCP server configs.
/// We want to warn people who recently added these fields but can remove this after a few months.
fn ensure_no_inline_bearer_tokens(value: &TomlValue) -> std::io::Result<()> {
    let Some(servers_table) = value.as_table() else {
        return Ok(());
    };

    for (server_name, server_value) in servers_table {
        if let Some(server_table) = server_value.as_table()
            && server_table.contains_key("bearer_token")
        {
            let message = format!(
                "mcp_servers.{server_name} uses unsupported `bearer_token`; set `bearer_token_env_var`."
            );
            return Err(std::io::Error::new(ErrorKind::InvalidData, message));
        }
    }

    Ok(())
}

pub(crate) fn set_project_trust_level_inner(
    doc: &mut DocumentMut,
    project_path: &Path,
    trust_level: TrustLevel,
) -> anyhow::Result<()> {
    // Ensure we render a human-friendly structure:
    //
    // [projects]
    // [projects."/path/to/project"]
    // trust_level = "trusted" or "untrusted"
    //
    // rather than inline tables like:
    //
    // [projects]
    // "/path/to/project" = { trust_level = "trusted" }
    let project_key = project_path.to_string_lossy().to_string();

    // Ensure top-level `projects` exists as a non-inline, explicit table. If it
    // exists but was previously represented as a non-table (e.g., inline),
    // replace it with an explicit table.
    {
        let root = doc.as_table_mut();
        // If `projects` exists but isn't a standard table (e.g., it's an inline table),
        // convert it to an explicit table while preserving existing entries.
        let existing_projects = root.get("projects").cloned();
        if existing_projects.as_ref().is_none_or(|i| !i.is_table()) {
            let mut projects_tbl = toml_edit::Table::new();
            projects_tbl.set_implicit(true);

            // If there was an existing inline table, migrate its entries to explicit tables.
            if let Some(inline_tbl) = existing_projects.as_ref().and_then(|i| i.as_inline_table()) {
                for (k, v) in inline_tbl.iter() {
                    if let Some(inner_tbl) = v.as_inline_table() {
                        let new_tbl = inner_tbl.clone().into_table();
                        projects_tbl.insert(k, toml_edit::Item::Table(new_tbl));
                    }
                }
            }

            root.insert("projects", toml_edit::Item::Table(projects_tbl));
        }
    }
    let Some(projects_tbl) = doc["projects"].as_table_mut() else {
        return Err(anyhow::anyhow!(
            "projects table missing after initialization"
        ));
    };

    // Ensure the per-project entry is its own explicit table. If it exists but
    // is not a table (e.g., an inline table), replace it with an explicit table.
    let needs_proj_table = !projects_tbl.contains_key(project_key.as_str())
        || projects_tbl
            .get(project_key.as_str())
            .and_then(|i| i.as_table())
            .is_none();
    if needs_proj_table {
        projects_tbl.insert(project_key.as_str(), toml_edit::table());
    }
    let Some(proj_tbl) = projects_tbl
        .get_mut(project_key.as_str())
        .and_then(|i| i.as_table_mut())
    else {
        return Err(anyhow::anyhow!("project table missing for {project_key}"));
    };
    proj_tbl.set_implicit(false);
    proj_tbl["trust_level"] = toml_edit::value(trust_level.to_string());
    Ok(())
}

/// Patch `CODEX_HOME/config.toml` project state to set trust level.
/// Use with caution.
pub fn set_project_trust_level(
    codex_home: &Path,
    project_path: &Path,
    trust_level: TrustLevel,
) -> anyhow::Result<()> {
    use crate::config::edit::ConfigEditsBuilder;

    ConfigEditsBuilder::new(codex_home)
        .set_project_trust_level(project_path, trust_level)
        .apply_blocking()
}

/// Save the default OSS provider preference to config.toml
pub fn set_default_oss_provider(codex_home: &Path, provider: &str) -> std::io::Result<()> {
    // Validate that the provider is one of the known OSS providers
    match provider {
        LMSTUDIO_OSS_PROVIDER_ID | OLLAMA_OSS_PROVIDER_ID | OLLAMA_CHAT_PROVIDER_ID => {
            // Valid provider, continue
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Invalid OSS provider '{provider}'. Must be one of: {LMSTUDIO_OSS_PROVIDER_ID}, {OLLAMA_OSS_PROVIDER_ID}, {OLLAMA_CHAT_PROVIDER_ID}"
                ),
            ));
        }
    }
    use toml_edit::value;

    let edits = [ConfigEdit::SetPath {
        segments: vec!["oss_provider".to_string()],
        value: value(provider),
    }];

    ConfigEditsBuilder::new(codex_home)
        .with_edits(edits)
        .apply_blocking()
        .map_err(|err| std::io::Error::other(format!("failed to persist config.toml: {err}")))
}

/// Model provider used when nothing is set in config.toml, a config profile,
/// or on the command line.
///
/// In this fork the built-in Open WebUI provider is the default, always. Owl is
/// meant to talk to the user's own Open WebUI server, so it must not quietly end
/// up on OpenAI because the home directory happens to hold something else: a
/// saved ChatGPT login, an exported `OPENAI_API_KEY`, or the ephemeral
/// `CODEX_API_KEY` are all ignored here. `OPENWEBUI_BASE_URL` still overrides
/// where the built-in provider points, and the upstream OpenAI provider remains
/// available to anyone who asks for it with `model_provider = "openai"` (or any
/// other id) in config.toml, a config profile, or `--provider` on the command
/// line.
pub fn default_model_provider_id(cfg: &ConfigToml) -> String {
    // An explicit `model_provider` in config.toml is the user's chosen
    // provider and wins over the default rule (matches how Config resolves
    // the provider from `ConfigToml.model_provider`).
    if let Some(provider) = &cfg.model_provider {
        return provider.clone();
    }

    OPENWEBUI_PROVIDER_ID.to_string()
}

#[cfg(test)]
mod default_provider_tests {
    use super::ConfigBuilder;
    use super::ConfigToml;
    use super::OPENWEBUI_PROVIDER_ID;
    use super::default_model_provider_id;
    use crate::model_provider_info::WireApi;
    use base64::Engine;
    use pretty_assertions::assert_eq;

    #[test]
    fn explicit_model_provider_wins_over_default_rule() {
        // A bare `model_provider = "ollama"` in config.toml must be respected
        // even when the default rule would otherwise pick Open WebUI; the
        // process environment cannot affect this path.
        let cfg = ConfigToml {
            model_provider: Some("ollama".to_string()),
            ..Default::default()
        };
        assert_eq!(default_model_provider_id(&cfg), "ollama".to_string());
    }

    #[test]
    fn openwebui_is_the_default_when_nothing_is_configured() {
        assert_eq!(
            default_model_provider_id(&ConfigToml::default()),
            OPENWEBUI_PROVIDER_ID
        );
    }

    /// A ChatGPT login left in the home directory by an upstream owl install
    /// must not steer owl onto OpenAI. The resolved provider stays the built-in
    /// Open WebUI one, which is what makes the connect screen appear on a
    /// machine that used to run owl with a ChatGPT sign-in.
    #[tokio::test]
    async fn saved_chatgpt_login_does_not_change_the_resolved_provider() -> std::io::Result<()> {
        let codex_home = tempfile::TempDir::new()?;
        std::fs::write(
            codex_home.path().join("auth.json"),
            fake_chatgpt_auth_json()?,
        )?;

        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await?;

        assert_eq!(config.model_provider_id, OPENWEBUI_PROVIDER_ID);
        assert_eq!(config.model_provider.wire_api, WireApi::Chat);
        Ok(())
    }

    /// The shape of a `codex login` ChatGPT `auth.json`, with tokens that are
    /// obviously fake. The id token is a real base64url JWT so the file parses
    /// the way a saved login would.
    fn fake_chatgpt_auth_json() -> Result<String, serde_json::Error> {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let header = serde_json::json!({"alg": "none", "typ": "JWT"});
        let claims = serde_json::json!({
            "email": "someone@example.com",
            "email_verified": true,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_user_id": "user-12345",
                "user_id": "user-12345",
            },
        });
        let header_b64 = b64(&serde_json::to_vec(&header)?);
        let claims_b64 = b64(&serde_json::to_vec(&claims)?);
        let signature_b64 = b64(b"signature");
        let id_token = format!("{header_b64}.{claims_b64}.{signature_b64}");

        serde_json::to_string(&serde_json::json!({
            "OPENAI_API_KEY": serde_json::Value::Null,
            "tokens": {
                "id_token": id_token,
                "access_token": "fake-access-token",
                "refresh_token": "fake-refresh-token",
                "account_id": "fake-account",
            },
        }))
    }
}

/// Base config deserialized from ~/.owl/config.toml.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConfigToml {
    /// Optional override of model selection.
    pub model: Option<String>,
    /// Review model override used by the `/review` feature.
    pub review_model: Option<String>,

    /// Provider to use from the model_providers map.
    pub model_provider: Option<String>,

    /// Size of the context window for the model, in tokens.
    pub model_context_window: Option<i64>,

    /// Token usage threshold triggering auto-compaction of conversation history.
    pub model_auto_compact_token_limit: Option<i64>,

    /// Default approval policy for executing commands.
    pub approval_policy: Option<AskForApproval>,

    #[serde(default)]
    pub shell_environment_policy: ShellEnvironmentPolicyToml,

    /// Sandbox mode to use.
    pub sandbox_mode: Option<SandboxMode>,

    /// Sandbox configuration to apply if `sandbox` is `WorkspaceWrite`.
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,

    /// Optional external command to spawn for end-user notifications.
    #[serde(default)]
    pub notify: Option<Vec<String>>,

    /// System instructions.
    pub instructions: Option<String>,

    /// Developer instructions inserted as a `developer` role message.
    #[serde(default)]
    pub developer_instructions: Option<String>,

    /// Optional path to a file containing model instructions that will override
    /// the built-in instructions for the selected model. Users are STRONGLY
    /// DISCOURAGED from using this field, as deviating from the instructions
    /// sanctioned by owl will likely degrade model performance.
    pub model_instructions_file: Option<AbsolutePathBuf>,

    /// Compact prompt used for history compaction.
    pub compact_prompt: Option<String>,

    /// Preferred backend for storing CLI auth credentials.
    /// file (default): Use a file in the owl home directory.
    /// keyring: Use an OS-specific keyring service.
    /// auto: Use the keyring if available, otherwise use a file.
    #[serde(default)]
    pub cli_auth_credentials_store: Option<AuthCredentialsStoreMode>,

    /// Definition for MCP servers that owl can reach out to for tool calls.
    #[serde(default)]
    // Uses the raw MCP input shape (custom deserialization) rather than `McpServerConfig`.
    #[schemars(schema_with = "crate::config::schema::mcp_servers_schema")]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Preferred backend for storing MCP OAuth credentials.
    /// keyring: Use an OS-specific keyring service.
    ///          https://github.com/openai/codex/blob/main/codex-rs/rmcp-client/src/oauth.rs#L2
    /// file: Use a file in the owl home directory.
    /// auto (default): Use the OS-specific keyring service if available, otherwise use a file.
    #[serde(default)]
    pub mcp_oauth_credentials_store: Option<OAuthCredentialsStoreMode>,

    /// Optional fixed port for the local HTTP callback server used during MCP OAuth login.
    /// When unset, owl will bind to an ephemeral port chosen by the OS.
    pub mcp_oauth_callback_port: Option<u16>,

    /// User-defined provider entries that extend/override the built-in list.
    #[serde(default)]
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Maximum number of bytes to include from an AGENTS.md project doc file.
    pub project_doc_max_bytes: Option<usize>,

    /// Ordered list of fallback filenames to look for when AGENTS.md is missing.
    pub project_doc_fallback_filenames: Option<Vec<String>>,

    /// Token budget applied when storing tool/function outputs in the context manager.
    pub tool_output_token_limit: Option<usize>,

    /// Profile to use from the `profiles` map.
    pub profile: Option<String>,

    /// Named profiles to facilitate switching between different configurations.
    #[serde(default)]
    pub profiles: HashMap<String, ConfigProfile>,

    /// Settings that govern if and what will be written to `~/.owl/history.jsonl`.
    #[serde(default)]
    pub history: Option<History>,

    /// Optional URI-based file opener. If set, citations to files in the model
    /// output will be hyperlinked using the specified URI scheme.
    pub file_opener: Option<UriBasedFileOpener>,

    /// Collection of settings that are specific to the TUI.
    pub tui: Option<Tui>,

    /// When set to `true`, `AgentReasoning` events will be hidden from the
    /// UI/output. Defaults to `false`.
    pub hide_agent_reasoning: Option<bool>,

    /// When set to `true`, `AgentReasoningRawContentEvent` events will be shown in the UI/output.
    /// Defaults to `false`.
    pub show_raw_agent_reasoning: Option<bool>,

    pub model_reasoning_effort: Option<ReasoningEffort>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    /// Optional verbosity control for GPT-5 models (Responses API `text.verbosity`).
    pub model_verbosity: Option<Verbosity>,

    /// Override to force-enable reasoning summaries for the configured model.
    pub model_supports_reasoning_summaries: Option<bool>,

    /// EXPERIMENTAL
    /// Optionally specify a personality for the model
    pub model_personality: Option<Personality>,

    pub projects: Option<HashMap<String, ProjectConfig>>,

    /// Controls the web search tool mode: disabled, cached, or live.
    pub web_search: Option<WebSearchMode>,

    /// Backend for the locally executed `web_search` tool (`[search]` table).
    pub search: Option<SearchToml>,

    /// Nested tools section for feature toggles
    pub tools: Option<ToolsToml>,

    /// Agent-related settings (thread limits, etc.).
    pub agents: Option<AgentsToml>,

    /// User-level skill config entries keyed by SKILL.md path.
    pub skills: Option<SkillsConfig>,

    /// Centralized feature flags (new). Prefer this over individual toggles.
    #[serde(default)]
    // Injects known feature keys into the schema and forbids unknown keys.
    #[schemars(schema_with = "crate::config::schema::features_schema")]
    pub features: Option<FeaturesToml>,

    /// Settings for ghost snapshots (used for undo).
    #[serde(default)]
    pub ghost_snapshot: Option<GhostSnapshotToml>,

    /// Markers used to detect the project root when searching parent
    /// directories for `.owl` folders. Defaults to [".git"] when unset.
    #[serde(default)]
    pub project_root_markers: Option<Vec<String>>,

    /// When true, disables burst-paste detection for typed input entirely.
    /// All characters are inserted as they are received, and no buffering
    /// or placeholder replacement will occur for fast keypress bursts.
    pub disable_paste_burst: Option<bool>,

    /// When `false`, disables analytics across owl product surfaces in this machine.
    /// Defaults to `true`.
    pub analytics: Option<crate::config::types::AnalyticsConfigToml>,

    /// When `false`, disables feedback collection across owl product surfaces.
    /// Defaults to `true`.
    pub feedback: Option<crate::config::types::FeedbackConfigToml>,

    /// OTEL configuration.
    pub otel: Option<crate::config::types::OtelConfigToml>,

    /// Tracks whether the Windows onboarding screen has been acknowledged.
    pub windows_wsl_setup_acknowledged: Option<bool>,

    /// Collection of in-product notices (different from notifications)
    /// See [`crate::config::types::Notices`] for more details
    pub notice: Option<Notice>,

    /// Legacy, now use features
    /// Deprecated: ignored. Use `model_instructions_file`.
    #[schemars(skip)]
    pub experimental_instructions_file: Option<AbsolutePathBuf>,
    pub experimental_compact_prompt_file: Option<AbsolutePathBuf>,
    pub experimental_use_unified_exec_tool: Option<bool>,
    pub experimental_use_freeform_apply_patch: Option<bool>,
    /// Preferred OSS provider for local models, e.g. "lmstudio", "ollama", or "ollama-chat".
    pub oss_provider: Option<String>,
}

impl From<ConfigToml> for UserSavedConfig {
    fn from(config_toml: ConfigToml) -> Self {
        let profiles = config_toml
            .profiles
            .into_iter()
            .map(|(k, v)| (k, v.into()))
            .collect();

        Self {
            approval_policy: config_toml.approval_policy,
            sandbox_mode: config_toml.sandbox_mode,
            sandbox_settings: config_toml.sandbox_workspace_write.map(From::from),
            model: config_toml.model,
            model_reasoning_effort: config_toml.model_reasoning_effort,
            model_reasoning_summary: config_toml.model_reasoning_summary,
            model_verbosity: config_toml.model_verbosity,
            tools: config_toml.tools.map(From::from),
            profile: config_toml.profile,
            profiles,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProjectConfig {
    pub trust_level: Option<TrustLevel>,
}

impl ProjectConfig {
    pub fn is_trusted(&self) -> bool {
        matches!(self.trust_level, Some(TrustLevel::Trusted))
    }

    pub fn is_untrusted(&self) -> bool {
        matches!(self.trust_level, Some(TrustLevel::Untrusted))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ToolsToml {
    #[serde(default, alias = "web_search_request")]
    pub web_search: Option<bool>,

    /// Enable the `view_image` tool that lets the agent attach local images.
    #[serde(default)]
    pub view_image: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SearchToml {
    /// GET search endpoint for the locally executed `web_search` tool, for
    /// example a self-hosted SearXNG `/search` URL. owl appends `q=<query>` and
    /// the entries of `params` to this URL.
    pub url: Option<String>,

    /// Name of an environment variable holding a bearer token to send with
    /// search requests. The variable must be set when this option is used.
    pub api_key_env: Option<String>,

    /// Extra query parameters appended to every search request, for example
    /// `format = "json"` for SearXNG JSON output.
    #[serde(default)]
    pub params: Option<BTreeMap<String, String>>,

    /// Maximum number of results to pass to the model. Defaults to 5.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub max_results: Option<usize>,

    /// Per-request timeout in milliseconds. Defaults to 15000.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub timeout_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AgentsToml {
    /// Maximum number of agent threads that can be open concurrently.
    /// When unset, no limit is enforced.
    #[schemars(range(min = 1))]
    pub max_threads: Option<usize>,
}

impl From<ToolsToml> for Tools {
    fn from(tools_toml: ToolsToml) -> Self {
        Self {
            web_search: tools_toml.web_search,
            view_image: tools_toml.view_image,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct GhostSnapshotToml {
    /// Exclude untracked files larger than this many bytes from ghost snapshots.
    #[serde(alias = "ignore_untracked_files_over_bytes")]
    pub ignore_large_untracked_files: Option<i64>,
    /// Ignore untracked directories that contain this many files or more.
    /// (Still emits a warning unless warnings are disabled.)
    #[serde(alias = "large_untracked_dir_warning_threshold")]
    pub ignore_large_untracked_dirs: Option<i64>,
    /// Disable all ghost snapshot warning events.
    pub disable_warnings: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SandboxPolicyResolution {
    pub policy: SandboxPolicy,
    pub forced_auto_mode_downgraded_on_windows: bool,
}

impl ConfigToml {
    /// Derive the effective sandbox policy from the configuration.
    fn derive_sandbox_policy(
        &self,
        sandbox_mode_override: Option<SandboxMode>,
        profile_sandbox_mode: Option<SandboxMode>,
        resolved_cwd: &Path,
    ) -> SandboxPolicyResolution {
        let resolved_sandbox_mode = sandbox_mode_override
            .or(profile_sandbox_mode)
            .or(self.sandbox_mode)
            .or_else(|| {
                // if no sandbox_mode is set, but user has marked directory as trusted or untrusted, use WorkspaceWrite
                self.get_active_project(resolved_cwd).and_then(|p| {
                    if p.is_trusted() || p.is_untrusted() {
                        Some(SandboxMode::WorkspaceWrite)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();
        let mut sandbox_policy = match resolved_sandbox_mode {
            SandboxMode::ReadOnly => SandboxPolicy::new_read_only_policy(),
            SandboxMode::WorkspaceWrite => match self.sandbox_workspace_write.as_ref() {
                Some(SandboxWorkspaceWrite {
                    writable_roots,
                    network_access,
                    exclude_tmpdir_env_var,
                    exclude_slash_tmp,
                }) => SandboxPolicy::WorkspaceWrite {
                    writable_roots: writable_roots.clone(),
                    network_access: *network_access,
                    exclude_tmpdir_env_var: *exclude_tmpdir_env_var,
                    exclude_slash_tmp: *exclude_slash_tmp,
                },
                None => SandboxPolicy::new_workspace_write_policy(),
            },
            SandboxMode::DangerFullAccess => SandboxPolicy::DangerFullAccess,
        };
        let mut forced_auto_mode_downgraded_on_windows = false;
        if cfg!(target_os = "windows")
            && matches!(resolved_sandbox_mode, SandboxMode::WorkspaceWrite)
            // If the experimental Windows sandbox is enabled, do not force a downgrade.
            && crate::safety::get_platform_sandbox().is_none()
        {
            sandbox_policy = SandboxPolicy::new_read_only_policy();
            forced_auto_mode_downgraded_on_windows = true;
        }
        SandboxPolicyResolution {
            policy: sandbox_policy,
            forced_auto_mode_downgraded_on_windows,
        }
    }

    /// Resolves the cwd to an existing project, or returns None if ConfigToml
    /// does not contain a project corresponding to cwd or a git repo for cwd
    pub fn get_active_project(&self, resolved_cwd: &Path) -> Option<ProjectConfig> {
        let projects = self.projects.clone().unwrap_or_default();

        if let Some(project_config) = projects.get(&resolved_cwd.to_string_lossy().to_string()) {
            return Some(project_config.clone());
        }

        // If cwd lives inside a git repo/worktree, check whether the root git project
        // (the primary repository working directory) is trusted. This lets
        // worktrees inherit trust from the main project.
        if let Some(repo_root) = resolve_root_git_project_for_trust(resolved_cwd)
            && let Some(project_config_for_root) =
                projects.get(&repo_root.to_string_lossy().to_string_lossy().to_string())
        {
            return Some(project_config_for_root.clone());
        }

        None
    }

    pub fn get_config_profile(
        &self,
        override_profile: Option<String>,
    ) -> Result<ConfigProfile, std::io::Error> {
        let profile = override_profile.or_else(|| self.profile.clone());

        match profile {
            Some(key) => {
                if let Some(profile) = self.profiles.get(key.as_str()) {
                    return Ok(profile.clone());
                }

                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("config profile `{key}` not found"),
                ))
            }
            None => Ok(ConfigProfile::default()),
        }
    }
}

/// Optional overrides for user configuration (e.g., from CLI flags).
#[derive(Default, Debug, Clone)]
pub struct ConfigOverrides {
    pub model: Option<String>,
    pub review_model: Option<String>,
    pub cwd: Option<PathBuf>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox_mode: Option<SandboxMode>,
    pub model_provider: Option<String>,
    pub config_profile: Option<String>,
    pub codex_linux_sandbox_exe: Option<PathBuf>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub model_personality: Option<Personality>,
    pub compact_prompt: Option<String>,
    pub include_apply_patch_tool: Option<bool>,
    pub show_raw_agent_reasoning: Option<bool>,
    pub tools_web_search_request: Option<bool>,
    pub ephemeral: Option<bool>,
    /// Additional directories that should be treated as writable roots for this session.
    pub additional_writable_roots: Vec<PathBuf>,
}

/// Resolves the OSS provider from CLI override, profile config, or global config.
/// Returns `None` if no provider is configured at any level.
pub fn resolve_oss_provider(
    explicit_provider: Option<&str>,
    config_toml: &ConfigToml,
    config_profile: Option<String>,
) -> Option<String> {
    if let Some(provider) = explicit_provider {
        // Explicit provider specified (e.g., via --local-provider)
        Some(provider.to_string())
    } else {
        // Check profile config first, then global config
        let profile = config_toml.get_config_profile(config_profile).ok();
        if let Some(profile) = &profile {
            // Check if profile has an oss provider
            if let Some(profile_oss_provider) = &profile.oss_provider {
                Some(profile_oss_provider.clone())
            }
            // If not then check if the toml has an oss provider
            else {
                config_toml.oss_provider.clone()
            }
        } else {
            config_toml.oss_provider.clone()
        }
    }
}

/// Resolve the web search mode from explicit config and feature flags.
fn resolve_web_search_mode(
    config_toml: &ConfigToml,
    config_profile: &ConfigProfile,
    features: &Features,
) -> Option<WebSearchMode> {
    if let Some(mode) = config_profile.web_search.or(config_toml.web_search) {
        return Some(mode);
    }
    if features.enabled(Feature::WebSearchCached) {
        return Some(WebSearchMode::Cached);
    }
    if features.enabled(Feature::WebSearchRequest) {
        return Some(WebSearchMode::Live);
    }
    None
}

/// Resolve the `[search]` table into a [`WebSearchBackend`].
///
/// When web search is enabled the endpoint is required: owl runs the search
/// itself, so without a configured URL the tool could only ever fail, and that
/// is reported at config load time instead of at tool-call time.
fn resolve_web_search_backend(
    config_toml: &ConfigToml,
    web_search_mode: Option<WebSearchMode>,
) -> std::io::Result<Option<WebSearchBackend>> {
    let search_enabled = matches!(
        web_search_mode,
        Some(WebSearchMode::Cached) | Some(WebSearchMode::Live)
    );
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidData, message);

    let Some(search) = config_toml.search.clone() else {
        if search_enabled {
            return Err(invalid(
                "web_search is enabled but no search endpoint is configured; add `url` under `[search]` in config.toml".to_string(),
            ));
        }
        return Ok(None);
    };

    let raw_url = search.url.clone().filter(|url| !url.trim().is_empty());
    let Some(raw_url) = raw_url else {
        return Err(invalid(
            "the [search] table is present but `url` is missing; set `url` to your search endpoint or remove the section".to_string(),
        ));
    };
    let parsed_url = url::Url::parse(&raw_url)
        .map_err(|err| invalid(format!("[search] `url` is not a valid URL: {err}")))?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        return Err(invalid(format!(
            "[search] `url` must use http or https, got {:?}",
            parsed_url.scheme()
        )));
    }

    let api_key = match search.api_key_env.as_deref().filter(|name| !name.is_empty()) {
        Some(env_name) => Some(std::env::var(env_name).map_err(|_| {
            invalid(format!(
                "[search] `api_key_env` is set to {env_name:?} but that environment variable is not set"
            ))
        })?),
        None => None,
    };

    Ok(Some(WebSearchBackend {
        url: raw_url,
        api_key,
        params: search.params.unwrap_or_default(),
        max_results: search.max_results.unwrap_or(DEFAULT_WEB_SEARCH_MAX_RESULTS),
        timeout: search
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_WEB_SEARCH_TIMEOUT),
    }))
}

impl Config {
    #[cfg(test)]
    fn load_from_base_config_with_overrides(
        cfg: ConfigToml,
        overrides: ConfigOverrides,
        codex_home: PathBuf,
    ) -> std::io::Result<Self> {
        // Note this ignores requirements.toml enforcement for tests.
        let config_layer_stack = ConfigLayerStack::default();
        Self::load_config_with_layer_stack(cfg, overrides, codex_home, config_layer_stack)
    }

    fn load_config_with_layer_stack(
        cfg: ConfigToml,
        overrides: ConfigOverrides,
        codex_home: PathBuf,
        config_layer_stack: ConfigLayerStack,
    ) -> std::io::Result<Self> {
        let requirements = config_layer_stack.requirements().clone();
        let user_instructions = Self::load_instructions(Some(&codex_home));

        // Destructure ConfigOverrides fully to ensure all overrides are applied.
        let ConfigOverrides {
            model,
            review_model: override_review_model,
            cwd,
            approval_policy: approval_policy_override,
            sandbox_mode,
            model_provider,
            config_profile: config_profile_key,
            codex_linux_sandbox_exe,
            base_instructions,
            developer_instructions,
            model_personality,
            compact_prompt,
            include_apply_patch_tool: include_apply_patch_tool_override,
            show_raw_agent_reasoning,
            tools_web_search_request: override_tools_web_search_request,
            ephemeral,
            additional_writable_roots,
        } = overrides;

        let active_profile_name = config_profile_key
            .as_ref()
            .or(cfg.profile.as_ref())
            .cloned();
        let config_profile = match active_profile_name.as_ref() {
            Some(key) => cfg
                .profiles
                .get(key)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("config profile `{key}` not found"),
                    )
                })?
                .clone(),
            None => ConfigProfile::default(),
        };

        let feature_overrides = FeatureOverrides {
            include_apply_patch_tool: include_apply_patch_tool_override,
            web_search_request: override_tools_web_search_request,
        };

        let features = Features::from_config(&cfg, &config_profile, feature_overrides);
        let web_search_mode = resolve_web_search_mode(&cfg, &config_profile, &features);
        let web_search_backend = resolve_web_search_backend(&cfg, web_search_mode)?;
        #[cfg(target_os = "windows")]
        {
            // Base flag controls sandbox on/off; elevated only applies when base is enabled.
            let sandbox_enabled = features.enabled(Feature::WindowsSandbox);
            crate::safety::set_windows_sandbox_enabled(sandbox_enabled);
            let elevated_enabled =
                sandbox_enabled && features.enabled(Feature::WindowsSandboxElevated);
            crate::safety::set_windows_elevated_sandbox_enabled(elevated_enabled);
        }

        let resolved_cwd = {
            use std::env;

            match cwd {
                None => {
                    tracing::info!("cwd not set, using current dir");
                    env::current_dir()?
                }
                Some(p) if p.is_absolute() => p,
                Some(p) => {
                    // Resolve relative path against the current working directory.
                    tracing::info!("cwd is relative, resolving against current dir");
                    let mut current = env::current_dir()?;
                    current.push(p);
                    current
                }
            }
        };
        let additional_writable_roots: Vec<AbsolutePathBuf> = additional_writable_roots
            .into_iter()
            .map(|path| AbsolutePathBuf::resolve_path_against_base(path, &resolved_cwd))
            .collect::<Result<Vec<_>, _>>()?;
        let active_project = cfg
            .get_active_project(&resolved_cwd)
            .unwrap_or(ProjectConfig { trust_level: None });

        let SandboxPolicyResolution {
            policy: mut sandbox_policy,
            forced_auto_mode_downgraded_on_windows,
        } = cfg.derive_sandbox_policy(sandbox_mode, config_profile.sandbox_mode, &resolved_cwd);
        if let SandboxPolicy::WorkspaceWrite { writable_roots, .. } = &mut sandbox_policy {
            for path in additional_writable_roots {
                if !writable_roots.iter().any(|existing| existing == &path) {
                    writable_roots.push(path);
                }
            }
        }
        let approval_policy = approval_policy_override
            .or(config_profile.approval_policy)
            .or(cfg.approval_policy)
            .unwrap_or_else(|| {
                if active_project.is_trusted() {
                    AskForApproval::OnRequest
                } else if active_project.is_untrusted() {
                    AskForApproval::UnlessTrusted
                } else {
                    AskForApproval::default()
                }
            });
        // TODO(dylan): We should be able to leverage ConfigLayerStack so that
        // we can reliably check this at every config level.
        let did_user_set_custom_approval_policy_or_sandbox_mode = approval_policy_override
            .is_some()
            || config_profile.approval_policy.is_some()
            || cfg.approval_policy.is_some()
            || sandbox_mode.is_some()
            || config_profile.sandbox_mode.is_some()
            || cfg.sandbox_mode.is_some();

        let mut model_providers = built_in_model_providers();
        // Merge user-defined providers into the built-in list. User entries
        // win so that `[model_providers.openwebui]` (or any built-in id) can be
        // customized in config.toml.
        let default_model_provider = default_model_provider_id(&cfg);
        for (key, provider) in cfg.model_providers.into_iter() {
            model_providers.insert(key, provider);
        }

        let model_provider_id = model_provider
            .or(config_profile.model_provider)
            .or(cfg.model_provider)
            .unwrap_or(default_model_provider);
        let model_provider = model_providers
            .get(&model_provider_id)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Model provider `{model_provider_id}` not found"),
                )
            })?
            .clone();
        if model_provider.base_url.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Model provider `{model_provider_id}` needs a base_url; set it in [model_providers.{model_provider_id}] in config.toml"
                ),
            ));
        }

        let shell_environment_policy = cfg.shell_environment_policy.into();

        let history = cfg.history.unwrap_or_default();

        let agent_max_threads = cfg
            .agents
            .as_ref()
            .and_then(|agents| agents.max_threads)
            .or(DEFAULT_AGENT_MAX_THREADS);
        if agent_max_threads == Some(0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "agents.max_threads must be at least 1",
            ));
        }

        let ghost_snapshot = {
            let mut config = GhostSnapshotConfig::default();
            if let Some(ghost_snapshot) = cfg.ghost_snapshot.as_ref()
                && let Some(ignore_over_bytes) = ghost_snapshot.ignore_large_untracked_files
            {
                config.ignore_large_untracked_files = if ignore_over_bytes > 0 {
                    Some(ignore_over_bytes)
                } else {
                    None
                };
            }
            if let Some(ghost_snapshot) = cfg.ghost_snapshot.as_ref()
                && let Some(threshold) = ghost_snapshot.ignore_large_untracked_dirs
            {
                config.ignore_large_untracked_dirs =
                    if threshold > 0 { Some(threshold) } else { None };
            }
            if let Some(ghost_snapshot) = cfg.ghost_snapshot.as_ref()
                && let Some(disable_warnings) = ghost_snapshot.disable_warnings
            {
                config.disable_warnings = disable_warnings;
            }
            config
        };

        let include_apply_patch_tool_flag = features.enabled(Feature::ApplyPatchFreeform);
        let use_experimental_unified_exec_tool = features.enabled(Feature::UnifiedExec);

        let model = model.or(config_profile.model).or(cfg.model);

        let compact_prompt = compact_prompt.or(cfg.compact_prompt).and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        // Load base instructions override from a file if specified. If the
        // path is relative, resolve it against the effective cwd so the
        // behaviour matches other path-like config values.
        let model_instructions_path = config_profile
            .model_instructions_file
            .as_ref()
            .or(cfg.model_instructions_file.as_ref());
        let file_base_instructions =
            Self::try_read_non_empty_file(model_instructions_path, "model instructions file")?;
        let base_instructions = base_instructions.or(file_base_instructions);
        let developer_instructions = developer_instructions.or(cfg.developer_instructions);
        let model_personality = model_personality
            .or(config_profile.model_personality)
            .or(cfg.model_personality);

        let experimental_compact_prompt_path = config_profile
            .experimental_compact_prompt_file
            .as_ref()
            .or(cfg.experimental_compact_prompt_file.as_ref());
        let file_compact_prompt = Self::try_read_non_empty_file(
            experimental_compact_prompt_path,
            "experimental compact prompt file",
        )?;
        let compact_prompt = compact_prompt.or(file_compact_prompt);

        let review_model = override_review_model.or(cfg.review_model);

        // Ensure that every field of ConfigRequirements is applied to the final
        // Config.
        let ConfigRequirements {
            approval_policy: mut constrained_approval_policy,
            sandbox_policy: mut constrained_sandbox_policy,
            mcp_servers,
        } = requirements;

        constrained_approval_policy
            .set(approval_policy)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
        constrained_sandbox_policy
            .set(sandbox_policy)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;

        let mcp_servers = constrain_mcp_servers(cfg.mcp_servers.clone(), mcp_servers.as_ref())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;

        let config = Self {
            model,
            review_model,
            model_context_window: cfg.model_context_window,
            model_auto_compact_token_limit: cfg.model_auto_compact_token_limit,
            model_provider_id,
            model_provider,
            cwd: resolved_cwd,
            approval_policy: constrained_approval_policy,
            sandbox_policy: constrained_sandbox_policy,
            did_user_set_custom_approval_policy_or_sandbox_mode,
            forced_auto_mode_downgraded_on_windows,
            shell_environment_policy,
            notify: cfg.notify,
            user_instructions,
            base_instructions,
            model_personality,
            developer_instructions,
            compact_prompt,
            // The config.toml omits "_mode" because it's a config file. However, "_mode"
            // is important in code to differentiate the mode from the store implementation.
            cli_auth_credentials_store_mode: cfg.cli_auth_credentials_store.unwrap_or_default(),
            mcp_servers,
            // The config.toml omits "_mode" because it's a config file. However, "_mode"
            // is important in code to differentiate the mode from the store implementation.
            mcp_oauth_credentials_store_mode: cfg.mcp_oauth_credentials_store.unwrap_or_default(),
            mcp_oauth_callback_port: cfg.mcp_oauth_callback_port,
            model_providers,
            project_doc_max_bytes: cfg.project_doc_max_bytes.unwrap_or(PROJECT_DOC_MAX_BYTES),
            project_doc_fallback_filenames: cfg
                .project_doc_fallback_filenames
                .unwrap_or_default()
                .into_iter()
                .filter_map(|name| {
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect(),
            tool_output_token_limit: cfg.tool_output_token_limit,
            agent_max_threads,
            codex_home,
            config_layer_stack,
            history,
            ephemeral: ephemeral.unwrap_or_default(),
            file_opener: cfg.file_opener.unwrap_or(UriBasedFileOpener::VsCode),
            codex_linux_sandbox_exe,

            hide_agent_reasoning: cfg.hide_agent_reasoning.unwrap_or(false),
            show_raw_agent_reasoning: cfg
                .show_raw_agent_reasoning
                .or(show_raw_agent_reasoning)
                .unwrap_or(false),
            model_reasoning_effort: config_profile
                .model_reasoning_effort
                .or(cfg.model_reasoning_effort),
            model_reasoning_summary: config_profile
                .model_reasoning_summary
                .or(cfg.model_reasoning_summary)
                .unwrap_or_default(),
            model_supports_reasoning_summaries: cfg.model_supports_reasoning_summaries,
            model_verbosity: config_profile.model_verbosity.or(cfg.model_verbosity),
            include_apply_patch_tool: include_apply_patch_tool_flag,
            web_search_mode,
            web_search_backend,
            use_experimental_unified_exec_tool,
            ghost_snapshot,
            features,
            active_profile: active_profile_name,
            active_project,
            windows_wsl_setup_acknowledged: cfg.windows_wsl_setup_acknowledged.unwrap_or(false),
            notices: cfg.notice.unwrap_or_default(),
            disable_paste_burst: cfg.disable_paste_burst.unwrap_or(false),
            analytics_enabled: config_profile
                .analytics
                .as_ref()
                .and_then(|a| a.enabled)
                .or(cfg.analytics.as_ref().and_then(|a| a.enabled)),
            feedback_enabled: cfg
                .feedback
                .as_ref()
                .and_then(|feedback| feedback.enabled)
                .unwrap_or(true),
            tui_notifications: cfg
                .tui
                .as_ref()
                .map(|t| t.notifications.clone())
                .unwrap_or_default(),
            animations: cfg.tui.as_ref().map(|t| t.animations).unwrap_or(true),
            show_tooltips: cfg.tui.as_ref().map(|t| t.show_tooltips).unwrap_or(true),
            experimental_mode: cfg.tui.as_ref().and_then(|t| t.experimental_mode),
            tui_alternate_screen: cfg
                .tui
                .as_ref()
                .map(|t| t.alternate_screen)
                .unwrap_or_default(),
            otel: {
                let t: OtelConfigToml = cfg.otel.unwrap_or_default();
                let log_user_prompt = t.log_user_prompt.unwrap_or(false);
                let environment = t
                    .environment
                    .unwrap_or(DEFAULT_OTEL_ENVIRONMENT.to_string());
                let exporter = t.exporter.unwrap_or(OtelExporterKind::None);
                let trace_exporter = t.trace_exporter.unwrap_or_else(|| exporter.clone());
                OtelConfig {
                    log_user_prompt,
                    environment,
                    exporter,
                    trace_exporter,
                    metrics_exporter: OtelExporterKind::None,
                }
            },
        };
        Ok(config)
    }

    fn load_instructions(codex_dir: Option<&Path>) -> Option<String> {
        let base = codex_dir?;
        for candidate in [LOCAL_PROJECT_DOC_FILENAME, DEFAULT_PROJECT_DOC_FILENAME] {
            let mut path = base.to_path_buf();
            path.push(candidate);
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let trimmed = contents.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    /// If `path` is `Some`, attempts to read the file at the given path and
    /// returns its contents as a trimmed `String`. If the file is empty, or
    /// is `Some` but cannot be read, returns an `Err`.
    fn try_read_non_empty_file(
        path: Option<&AbsolutePathBuf>,
        context: &str,
    ) -> std::io::Result<Option<String>> {
        let Some(path) = path else {
            return Ok(None);
        };

        let contents = std::fs::read_to_string(path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to read {context} {}: {e}", path.display()),
            )
        })?;

        let s = contents.trim().to_string();
        if s.is_empty() {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{context} is empty: {}", path.display()),
            ))
        } else {
            Ok(Some(s))
        }
    }

    pub fn set_windows_sandbox_globally(&mut self, value: bool) {
        crate::safety::set_windows_sandbox_enabled(value);
        if value {
            self.features.enable(Feature::WindowsSandbox);
        } else {
            self.features.disable(Feature::WindowsSandbox);
        }
        self.forced_auto_mode_downgraded_on_windows = !value;
    }

    pub fn set_windows_elevated_sandbox_globally(&mut self, value: bool) {
        crate::safety::set_windows_elevated_sandbox_enabled(value);
        if value {
            self.features.enable(Feature::WindowsSandboxElevated);
        } else {
            self.features.disable(Feature::WindowsSandboxElevated);
        }
    }
}

pub(crate) fn uses_deprecated_instructions_file(config_layer_stack: &ConfigLayerStack) -> bool {
    config_layer_stack
        .layers_high_to_low()
        .into_iter()
        .any(|layer| toml_uses_deprecated_instructions_file(&layer.config))
}

fn toml_uses_deprecated_instructions_file(value: &TomlValue) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    if table.contains_key("experimental_instructions_file") {
        return true;
    }
    let Some(profiles) = table.get("profiles").and_then(TomlValue::as_table) else {
        return false;
    };
    profiles.values().any(|profile| {
        profile.as_table().is_some_and(|profile_table| {
            profile_table.contains_key("experimental_instructions_file")
        })
    })
}

/// Returns the path to owl's settings home, which owl keeps to itself.
///
/// `OWL_HOME` points it somewhere else; `CODEX_HOME` is still honored so existing
/// scripts and test harnesses that isolate a home keep working. With neither set
/// owl uses `~/.owl`, so a owl install on the same machine can never hand it a
/// config, a saved ChatGPT login or a model cache it was not meant to read.
///
/// - If `OWL_HOME` or `CODEX_HOME` is set, the value will be canonicalized and this
///   function will Err if the path does not exist.
/// - Otherwise this function does not verify that the directory exists.
pub fn find_codex_home() -> std::io::Result<PathBuf> {
    settings_home_from(
        std::env::var_os(OWL_HOME_ENV_VAR),
        std::env::var_os("CODEX_HOME"),
    )
}

/// The precedence behind [`find_codex_home`], with the environment values handed in
/// so it can be tested without touching the process environment.
///
/// owl keeps its own settings home: `OWL_HOME`, then `CODEX_HOME`, then `~/.owl`.
fn settings_home_from(
    owl_home: Option<std::ffi::OsString>,
    codex_home: Option<std::ffi::OsString>,
) -> std::io::Result<PathBuf> {
    // Honor an explicit home when one is set, `OWL_HOME` first.
    for value in owl_home.into_iter().chain(codex_home) {
        if !value.is_empty() {
            return PathBuf::from(value).canonicalize();
        }
    }

    let mut p = home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not find home directory",
        )
    })?;
    p.push(OWL_HOME_DIR_NAME);
    Ok(p)
}

/// Returns the path to the folder where owl logs are stored. Does not verify
/// that the directory exists.
pub fn log_dir(cfg: &Config) -> std::io::Result<PathBuf> {
    let mut p = cfg.codex_home.clone();
    p.push("log");
    Ok(p)
}

/// owl keeps its own settings home; these tests pin that down because reading
/// another tool's home is exactly how owl ended up serving ChatGPT model names.
#[cfg(test)]
mod owl_home_tests {
    use super::OWL_HOME_DIR_NAME;
    use super::settings_home_from;
    use dirs::home_dir;
    use pretty_assertions::assert_eq;
    use std::ffi::OsString;
    use tempfile::tempdir;

    fn as_value(path: &std::path::Path) -> OsString {
        path.as_os_str().to_os_string()
    }

    #[test]
    fn the_settings_home_is_dot_owl_and_never_dot_codex() {
        let home = home_dir().expect("home directory");

        let resolved = settings_home_from(None, None).expect("resolve the owl settings home");

        assert_eq!(resolved, home.join(OWL_HOME_DIR_NAME));
        assert_ne!(resolved, home.join(".codex"));
    }

    #[test]
    fn owl_home_overrides_the_settings_home() {
        let owl_dir = tempdir().expect("owl home");
        let codex_dir = tempdir().expect("codex home");

        let resolved = settings_home_from(
            Some(as_value(owl_dir.path())),
            Some(as_value(codex_dir.path())),
        )
        .expect("resolve the owl settings home");

        assert_eq!(resolved, owl_dir.path());
    }

    #[test]
    fn codex_home_still_isolates_the_settings_home() {
        let codex_dir = tempdir().expect("codex home");

        let resolved = settings_home_from(None, Some(as_value(codex_dir.path())))
            .expect("resolve the owl settings home");

        assert_eq!(resolved, codex_dir.path());
    }

    #[test]
    fn an_empty_environment_value_falls_back_to_dot_owl() {
        let home = home_dir().expect("home directory");

        let resolved = settings_home_from(Some(OsString::new()), Some(OsString::new()))
            .expect("resolve the owl settings home");

        assert_eq!(resolved, home.join(OWL_HOME_DIR_NAME));
    }
}

#[cfg(test)]
mod tests {
    use crate::config::edit::ConfigEdit;
    use crate::config::edit::ConfigEditsBuilder;
    use crate::config::edit::apply_blocking;
    use crate::config::types::FeedbackConfigToml;
    use crate::config::types::HistoryPersistence;
    use crate::config::types::McpServerTransportConfig;
    use crate::config::types::Notifications;
    use crate::config_loader::RequirementSource;
    use crate::features::Feature;

    use super::*;
    use core_test_support::test_absolute_path;
    use pretty_assertions::assert_eq;

    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::time::Duration;
    use tempfile::TempDir;

    fn stdio_mcp(command: &str) -> McpServerConfig {
        McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: command.to_string(),
                args: Vec::new(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            enabled: true,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            enabled_tools: None,
            disabled_tools: None,
        }
    }

    fn http_mcp(url: &str) -> McpServerConfig {
        McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: url.to_string(),
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
            },
            enabled: true,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            enabled_tools: None,
            disabled_tools: None,
        }
    }

    #[test]
    fn test_toml_parsing() {
        let history_with_persistence = r#"
[history]
persistence = "save-all"
"#;
        let history_with_persistence_cfg = toml::from_str::<ConfigToml>(history_with_persistence)
            .expect("TOML deserialization should succeed");
        assert_eq!(
            Some(History {
                persistence: HistoryPersistence::SaveAll,
                max_bytes: None,
            }),
            history_with_persistence_cfg.history
        );

        let history_no_persistence = r#"
[history]
persistence = "none"
"#;

        let history_no_persistence_cfg = toml::from_str::<ConfigToml>(history_no_persistence)
            .expect("TOML deserialization should succeed");
        assert_eq!(
            Some(History {
                persistence: HistoryPersistence::None,
                max_bytes: None,
            }),
            history_no_persistence_cfg.history
        );
    }

    #[test]
    fn tui_config_missing_notifications_field_defaults_to_enabled() {
        let cfg = r#"
[tui]
"#;

        let parsed = toml::from_str::<ConfigToml>(cfg)
            .expect("TUI config without notifications should succeed");
        let tui = parsed.tui.expect("config should include tui section");

        assert_eq!(
            tui,
            Tui {
                notifications: Notifications::Enabled(true),
                animations: true,
                show_tooltips: true,
                experimental_mode: None,
                alternate_screen: AltScreenMode::Auto,
            }
        );
    }

    #[test]
    fn test_sandbox_config_parsing() {
        let sandbox_full_access = r#"
sandbox_mode = "danger-full-access"

[sandbox_workspace_write]
network_access = false  # This should be ignored.
"#;
        let sandbox_full_access_cfg = toml::from_str::<ConfigToml>(sandbox_full_access)
            .expect("TOML deserialization should succeed");
        let sandbox_mode_override = None;
        let resolution = sandbox_full_access_cfg.derive_sandbox_policy(
            sandbox_mode_override,
            None,
            &PathBuf::from("/tmp/test"),
        );
        assert_eq!(
            resolution,
            SandboxPolicyResolution {
                policy: SandboxPolicy::DangerFullAccess,
                forced_auto_mode_downgraded_on_windows: false,
            }
        );

        let sandbox_read_only = r#"
sandbox_mode = "read-only"

[sandbox_workspace_write]
network_access = true  # This should be ignored.
"#;

        let sandbox_read_only_cfg = toml::from_str::<ConfigToml>(sandbox_read_only)
            .expect("TOML deserialization should succeed");
        let sandbox_mode_override = None;
        let resolution = sandbox_read_only_cfg.derive_sandbox_policy(
            sandbox_mode_override,
            None,
            &PathBuf::from("/tmp/test"),
        );
        assert_eq!(
            resolution,
            SandboxPolicyResolution {
                policy: SandboxPolicy::ReadOnly,
                forced_auto_mode_downgraded_on_windows: false,
            }
        );

        let writable_root = test_absolute_path("/my/workspace");
        let sandbox_workspace_write = format!(
            r#"
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
writable_roots = [
    {},
]
exclude_tmpdir_env_var = true
exclude_slash_tmp = true
"#,
            serde_json::json!(writable_root)
        );

        let sandbox_workspace_write_cfg = toml::from_str::<ConfigToml>(&sandbox_workspace_write)
            .expect("TOML deserialization should succeed");
        let sandbox_mode_override = None;
        let resolution = sandbox_workspace_write_cfg.derive_sandbox_policy(
            sandbox_mode_override,
            None,
            &PathBuf::from("/tmp/test"),
        );
        if cfg!(target_os = "windows") {
            assert_eq!(
                resolution,
                SandboxPolicyResolution {
                    policy: SandboxPolicy::ReadOnly,
                    forced_auto_mode_downgraded_on_windows: true,
                }
            );
        } else {
            assert_eq!(
                resolution,
                SandboxPolicyResolution {
                    policy: SandboxPolicy::WorkspaceWrite {
                        writable_roots: vec![writable_root.clone()],
                        network_access: false,
                        exclude_tmpdir_env_var: true,
                        exclude_slash_tmp: true,
                    },
                    forced_auto_mode_downgraded_on_windows: false,
                }
            );
        }

        let sandbox_workspace_write = format!(
            r#"
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
writable_roots = [
    {},
]
exclude_tmpdir_env_var = true
exclude_slash_tmp = true

[projects."/tmp/test"]
trust_level = "trusted"
"#,
            serde_json::json!(writable_root)
        );

        let sandbox_workspace_write_cfg = toml::from_str::<ConfigToml>(&sandbox_workspace_write)
            .expect("TOML deserialization should succeed");
        let sandbox_mode_override = None;
        let resolution = sandbox_workspace_write_cfg.derive_sandbox_policy(
            sandbox_mode_override,
            None,
            &PathBuf::from("/tmp/test"),
        );
        if cfg!(target_os = "windows") {
            assert_eq!(
                resolution,
                SandboxPolicyResolution {
                    policy: SandboxPolicy::ReadOnly,
                    forced_auto_mode_downgraded_on_windows: true,
                }
            );
        } else {
            assert_eq!(
                resolution,
                SandboxPolicyResolution {
                    policy: SandboxPolicy::WorkspaceWrite {
                        writable_roots: vec![writable_root],
                        network_access: false,
                        exclude_tmpdir_env_var: true,
                        exclude_slash_tmp: true,
                    },
                    forced_auto_mode_downgraded_on_windows: false,
                }
            );
        }
    }

    #[test]
    fn filter_mcp_servers_by_allowlist_enforces_identity_rules() {
        const MISMATCHED_COMMAND_SERVER: &str = "mismatched-command-should-disable";
        const MISMATCHED_URL_SERVER: &str = "mismatched-url-should-disable";
        const MATCHED_COMMAND_SERVER: &str = "matched-command-should-allow";
        const MATCHED_URL_SERVER: &str = "matched-url-should-allow";
        const DIFFERENT_NAME_SERVER: &str = "different-name-should-disable";

        const GOOD_CMD: &str = "good-cmd";
        const GOOD_URL: &str = "https://example.com/good";

        let mut servers = HashMap::from([
            (MISMATCHED_COMMAND_SERVER.to_string(), stdio_mcp("docs-cmd")),
            (
                MISMATCHED_URL_SERVER.to_string(),
                http_mcp("https://example.com/mcp"),
            ),
            (MATCHED_COMMAND_SERVER.to_string(), stdio_mcp(GOOD_CMD)),
            (MATCHED_URL_SERVER.to_string(), http_mcp(GOOD_URL)),
            (DIFFERENT_NAME_SERVER.to_string(), stdio_mcp("same-cmd")),
        ]);
        let source = RequirementSource::LegacyManagedConfigTomlFromMdm;
        let requirements = Sourced::new(
            BTreeMap::from([
                (
                    MISMATCHED_URL_SERVER.to_string(),
                    McpServerRequirement {
                        identity: McpServerIdentity::Url {
                            url: "https://example.com/other".to_string(),
                        },
                    },
                ),
                (
                    MISMATCHED_COMMAND_SERVER.to_string(),
                    McpServerRequirement {
                        identity: McpServerIdentity::Command {
                            command: "other-cmd".to_string(),
                        },
                    },
                ),
                (
                    MATCHED_URL_SERVER.to_string(),
                    McpServerRequirement {
                        identity: McpServerIdentity::Url {
                            url: GOOD_URL.to_string(),
                        },
                    },
                ),
                (
                    MATCHED_COMMAND_SERVER.to_string(),
                    McpServerRequirement {
                        identity: McpServerIdentity::Command {
                            command: GOOD_CMD.to_string(),
                        },
                    },
                ),
            ]),
            source.clone(),
        );
        filter_mcp_servers_by_requirements(&mut servers, Some(&requirements));

        let reason = Some(McpServerDisabledReason::Requirements { source });
        assert_eq!(
            servers
                .iter()
                .map(|(name, server)| (
                    name.clone(),
                    (server.enabled, server.disabled_reason.clone())
                ))
                .collect::<HashMap<String, (bool, Option<McpServerDisabledReason>)>>(),
            HashMap::from([
                (MISMATCHED_URL_SERVER.to_string(), (false, reason.clone())),
                (
                    MISMATCHED_COMMAND_SERVER.to_string(),
                    (false, reason.clone()),
                ),
                (MATCHED_URL_SERVER.to_string(), (true, None)),
                (MATCHED_COMMAND_SERVER.to_string(), (true, None)),
                (DIFFERENT_NAME_SERVER.to_string(), (false, reason)),
            ])
        );
    }

    #[test]
    fn filter_mcp_servers_by_allowlist_allows_all_when_unset() {
        let mut servers = HashMap::from([
            ("server-a".to_string(), stdio_mcp("cmd-a")),
            ("server-b".to_string(), http_mcp("https://example.com/b")),
        ]);

        filter_mcp_servers_by_requirements(&mut servers, None);

        assert_eq!(
            servers
                .iter()
                .map(|(name, server)| (
                    name.clone(),
                    (server.enabled, server.disabled_reason.clone())
                ))
                .collect::<HashMap<String, (bool, Option<McpServerDisabledReason>)>>(),
            HashMap::from([
                ("server-a".to_string(), (true, None)),
                ("server-b".to_string(), (true, None)),
            ])
        );
    }

    #[test]
    fn filter_mcp_servers_by_allowlist_blocks_all_when_empty() {
        let mut servers = HashMap::from([
            ("server-a".to_string(), stdio_mcp("cmd-a")),
            ("server-b".to_string(), http_mcp("https://example.com/b")),
        ]);

        let source = RequirementSource::LegacyManagedConfigTomlFromMdm;
        let requirements = Sourced::new(BTreeMap::new(), source.clone());
        filter_mcp_servers_by_requirements(&mut servers, Some(&requirements));

        let reason = Some(McpServerDisabledReason::Requirements { source });
        assert_eq!(
            servers
                .iter()
                .map(|(name, server)| (
                    name.clone(),
                    (server.enabled, server.disabled_reason.clone())
                ))
                .collect::<HashMap<String, (bool, Option<McpServerDisabledReason>)>>(),
            HashMap::from([
                ("server-a".to_string(), (false, reason.clone())),
                ("server-b".to_string(), (false, reason)),
            ])
        );
    }

    #[test]
    fn add_dir_override_extends_workspace_writable_roots() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let frontend = temp_dir.path().join("frontend");
        let backend = temp_dir.path().join("backend");
        std::fs::create_dir_all(&frontend)?;
        std::fs::create_dir_all(&backend)?;

        let overrides = ConfigOverrides {
            cwd: Some(frontend),
            sandbox_mode: Some(SandboxMode::WorkspaceWrite),
            additional_writable_roots: vec![PathBuf::from("../backend"), backend.clone()],
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            ConfigToml::default(),
            overrides,
            temp_dir.path().to_path_buf(),
        )?;

        let expected_backend = AbsolutePathBuf::try_from(backend).unwrap();
        if cfg!(target_os = "windows") {
            assert!(
                config.forced_auto_mode_downgraded_on_windows,
                "expected workspace-write request to be downgraded on Windows"
            );
            match config.sandbox_policy.get() {
                &SandboxPolicy::ReadOnly => {}
                other => panic!("expected read-only policy on Windows, got {other:?}"),
            }
        } else {
            match config.sandbox_policy.get() {
                SandboxPolicy::WorkspaceWrite { writable_roots, .. } => {
                    assert_eq!(
                        writable_roots
                            .iter()
                            .filter(|root| **root == expected_backend)
                            .count(),
                        1,
                        "expected single writable root entry for {}",
                        expected_backend.display()
                    );
                }
                other => panic!("expected workspace-write policy, got {other:?}"),
            }
        }

        Ok(())
    }

    #[test]
    fn config_defaults_to_file_cli_auth_store_mode() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml::default();

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.cli_auth_credentials_store_mode,
            AuthCredentialsStoreMode::File,
        );

        Ok(())
    }

    #[test]
    fn config_honors_explicit_keyring_auth_store_mode() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml {
            cli_auth_credentials_store: Some(AuthCredentialsStoreMode::Keyring),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.cli_auth_credentials_store_mode,
            AuthCredentialsStoreMode::Keyring,
        );

        Ok(())
    }

    #[test]
    fn config_defaults_to_auto_oauth_store_mode() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml::default();

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.mcp_oauth_credentials_store_mode,
            OAuthCredentialsStoreMode::Auto,
        );

        Ok(())
    }

    #[test]
    fn feedback_enabled_defaults_to_true() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml {
            feedback: Some(FeedbackConfigToml::default()),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(config.feedback_enabled, true);

        Ok(())
    }

    #[test]
    fn web_search_mode_uses_none_if_unset() {
        let cfg = ConfigToml::default();
        let profile = ConfigProfile::default();
        let features = Features::with_defaults();

        assert_eq!(resolve_web_search_mode(&cfg, &profile, &features), None);
    }

    #[test]
    fn web_search_mode_prefers_profile_over_legacy_flags() {
        let cfg = ConfigToml::default();
        let profile = ConfigProfile {
            web_search: Some(WebSearchMode::Live),
            ..Default::default()
        };
        let mut features = Features::with_defaults();
        features.enable(Feature::WebSearchCached);

        assert_eq!(
            resolve_web_search_mode(&cfg, &profile, &features),
            Some(WebSearchMode::Live)
        );
    }

    #[test]
    fn web_search_mode_disabled_overrides_legacy_request() {
        let cfg = ConfigToml {
            web_search: Some(WebSearchMode::Disabled),
            ..Default::default()
        };
        let profile = ConfigProfile::default();
        let mut features = Features::with_defaults();
        features.enable(Feature::WebSearchRequest);

        assert_eq!(
            resolve_web_search_mode(&cfg, &profile, &features),
            Some(WebSearchMode::Disabled)
        );
    }

    #[test]
    fn web_search_backend_none_when_unset() {
        let cfg = ConfigToml::default();
        assert!(resolve_web_search_backend(&cfg, None).unwrap().is_none());
    }

    #[test]
    fn web_search_backend_requires_url_when_mode_enabled() {
        let cfg = ConfigToml::default();
        let err = resolve_web_search_backend(&cfg, Some(WebSearchMode::Live))
            .expect_err("enabled web search without an endpoint should fail");
        let message = err.to_string();
        assert!(
            message.contains("web_search is enabled but no search endpoint is configured"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn web_search_backend_resolves_search_table_with_defaults() {
        let cfg = ConfigToml {
            search: Some(SearchToml {
                url: Some("http://127.0.0.1:8888/search".to_string()),
                params: Some(BTreeMap::from([("format".to_string(), "json".to_string())])),
                ..Default::default()
            }),
            ..Default::default()
        };

        let backend = resolve_web_search_backend(&cfg, Some(WebSearchMode::Live))
            .expect("search table should resolve")
            .expect("backend present");
        assert_eq!(backend.url, "http://127.0.0.1:8888/search");
        assert_eq!(backend.api_key, None);
        assert_eq!(
            backend.params,
            BTreeMap::from([("format".to_string(), "json".to_string())])
        );
        assert_eq!(backend.max_results, DEFAULT_WEB_SEARCH_MAX_RESULTS);
        assert_eq!(backend.timeout, DEFAULT_WEB_SEARCH_TIMEOUT);
    }

    #[test]
    fn web_search_backend_honors_explicit_limits() {
        let cfg = ConfigToml {
            search: Some(SearchToml {
                url: Some("https://search.example.test/search".to_string()),
                max_results: Some(2),
                timeout_ms: Some(900),
                ..Default::default()
            }),
            ..Default::default()
        };

        let backend = resolve_web_search_backend(&cfg, Some(WebSearchMode::Cached))
            .expect("search table should resolve")
            .expect("backend present");
        assert_eq!(backend.max_results, 2);
        assert_eq!(backend.timeout, Duration::from_millis(900));
    }

    #[test]
    fn web_search_backend_rejects_search_table_without_url() {
        let cfg = ConfigToml {
            search: Some(SearchToml::default()),
            ..Default::default()
        };
        let err =
            resolve_web_search_backend(&cfg, None).expect_err("empty [search] table should fail");
        assert!(
            err.to_string()
                .contains("the [search] table is present but `url` is missing"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn web_search_backend_rejects_invalid_urls() {
        let invalid_url = |url: &str| {
            let cfg = ConfigToml {
                search: Some(SearchToml {
                    url: Some(url.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            resolve_web_search_backend(&cfg, None)
                .expect_err("invalid url should fail")
                .to_string()
        };

        assert!(invalid_url("not a url").contains("not a valid URL"));
        assert!(invalid_url("ftp://example.test/search").contains("must use http or https"));
    }

    #[test]
    fn web_search_backend_requires_api_key_env_var_when_set() {
        let cfg = ConfigToml {
            search: Some(SearchToml {
                url: Some("http://127.0.0.1:8888/search".to_string()),
                api_key_env: Some("OWL_TEST_UNSET_SEARCH_API_KEY".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = resolve_web_search_backend(&cfg, Some(WebSearchMode::Live))
            .expect_err("missing api key env var should fail");
        let message = err.to_string();
        assert!(
            message.contains("OWL_TEST_UNSET_SEARCH_API_KEY")
                && message.contains("environment variable is not set"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn profile_legacy_toggles_override_base() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let mut profiles = HashMap::new();
        profiles.insert(
            "work".to_string(),
            ConfigProfile {
                tools_web_search: Some(false),
                ..Default::default()
            },
        );
        let cfg = ConfigToml {
            profiles,
            profile: Some("work".to_string()),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert!(!config.features.enabled(Feature::WebSearchRequest));

        Ok(())
    }

    #[tokio::test]
    async fn project_profile_overrides_user_profile() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let workspace = TempDir::new()?;
        let workspace_key = workspace.path().to_string_lossy().replace('\\', "\\\\");
        std::fs::write(
            codex_home.path().join(CONFIG_TOML_FILE),
            format!(
                r#"
profile = "global"

[profiles.global]
model = "global-model"

[profiles.project]
model = "project-model"

[projects."{workspace_key}"]
trust_level = "trusted"
"#,
            ),
        )?;
        let project_config_dir = workspace.path().join(".owl");
        std::fs::create_dir_all(&project_config_dir)?;
        std::fs::write(
            project_config_dir.join(CONFIG_TOML_FILE),
            r#"
profile = "project"
"#,
        )?;

        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                cwd: Some(workspace.path().to_path_buf()),
                ..Default::default()
            })
            .build()
            .await?;

        assert_eq!(config.active_profile.as_deref(), Some("project"));
        assert_eq!(config.model.as_deref(), Some("project-model"));

        Ok(())
    }

    #[test]
    fn profile_sandbox_mode_overrides_base() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let mut profiles = HashMap::new();
        profiles.insert(
            "work".to_string(),
            ConfigProfile {
                sandbox_mode: Some(SandboxMode::DangerFullAccess),
                ..Default::default()
            },
        );
        let cfg = ConfigToml {
            profiles,
            profile: Some("work".to_string()),
            sandbox_mode: Some(SandboxMode::ReadOnly),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert!(matches!(
            config.sandbox_policy.get(),
            &SandboxPolicy::DangerFullAccess
        ));
        assert!(config.did_user_set_custom_approval_policy_or_sandbox_mode);

        Ok(())
    }

    #[test]
    fn cli_override_takes_precedence_over_profile_sandbox_mode() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let mut profiles = HashMap::new();
        profiles.insert(
            "work".to_string(),
            ConfigProfile {
                sandbox_mode: Some(SandboxMode::DangerFullAccess),
                ..Default::default()
            },
        );
        let cfg = ConfigToml {
            profiles,
            profile: Some("work".to_string()),
            ..Default::default()
        };

        let overrides = ConfigOverrides {
            sandbox_mode: Some(SandboxMode::WorkspaceWrite),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            overrides,
            codex_home.path().to_path_buf(),
        )?;

        if cfg!(target_os = "windows") {
            assert!(matches!(
                config.sandbox_policy.get(),
                SandboxPolicy::ReadOnly
            ));
            assert!(config.forced_auto_mode_downgraded_on_windows);
        } else {
            assert!(matches!(
                config.sandbox_policy.get(),
                SandboxPolicy::WorkspaceWrite { .. }
            ));
            assert!(!config.forced_auto_mode_downgraded_on_windows);
        }

        Ok(())
    }

    #[test]
    fn feature_table_overrides_legacy_flags() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let mut entries = BTreeMap::new();
        entries.insert("apply_patch_freeform".to_string(), false);
        let cfg = ConfigToml {
            features: Some(crate::features::FeaturesToml { entries }),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert!(!config.features.enabled(Feature::ApplyPatchFreeform));
        assert!(!config.include_apply_patch_tool);

        Ok(())
    }

    #[test]
    fn legacy_toggles_map_to_features() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml {
            experimental_use_unified_exec_tool: Some(true),
            experimental_use_freeform_apply_patch: Some(true),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert!(config.features.enabled(Feature::ApplyPatchFreeform));
        assert!(config.features.enabled(Feature::UnifiedExec));

        assert!(config.include_apply_patch_tool);

        assert!(config.use_experimental_unified_exec_tool);

        Ok(())
    }
    #[test]
    fn config_honors_explicit_file_oauth_store_mode() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let cfg = ConfigToml {
            mcp_oauth_credentials_store: Some(OAuthCredentialsStoreMode::File),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.mcp_oauth_credentials_store_mode,
            OAuthCredentialsStoreMode::File,
        );

        Ok(())
    }

    #[tokio::test]
    async fn managed_config_overrides_oauth_store_mode() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let managed_path = codex_home.path().join("managed_config.toml");
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        std::fs::write(&config_path, "mcp_oauth_credentials_store = \"file\"\n")?;
        std::fs::write(&managed_path, "mcp_oauth_credentials_store = \"keyring\"\n")?;

        let overrides = LoaderOverrides {
            managed_config_path: Some(managed_path.clone()),
            #[cfg(target_os = "macos")]
            managed_preferences_base64: None,
            macos_managed_config_requirements_base64: None,
        };

        let cwd = AbsolutePathBuf::try_from(codex_home.path())?;
        let config_layer_stack =
            load_config_layers_state(codex_home.path(), Some(cwd), &Vec::new(), overrides).await?;
        let cfg = deserialize_config_toml_with_base(
            config_layer_stack.effective_config(),
            codex_home.path(),
        )
        .map_err(|e| {
            tracing::error!("Failed to deserialize overridden config: {e}");
            e
        })?;
        assert_eq!(
            cfg.mcp_oauth_credentials_store,
            Some(OAuthCredentialsStoreMode::Keyring),
        );

        let final_config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;
        assert_eq!(
            final_config.mcp_oauth_credentials_store_mode,
            OAuthCredentialsStoreMode::Keyring,
        );

        Ok(())
    }

    #[tokio::test]
    async fn load_global_mcp_servers_returns_empty_if_missing() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = load_global_mcp_servers(codex_home.path()).await?;
        assert!(servers.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_round_trips_entries() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let mut servers = BTreeMap::new();
        servers.insert(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "echo".to_string(),
                    args: vec!["hello".to_string()],
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: Some(Duration::from_secs(3)),
                tool_timeout_sec: Some(Duration::from_secs(5)),
                enabled_tools: None,
                disabled_tools: None,
            },
        );

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        assert_eq!(loaded.len(), 1);
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::Stdio {
                command,
                args,
                env,
                env_vars,
                cwd,
            } => {
                assert_eq!(command, "echo");
                assert_eq!(args, &vec!["hello".to_string()]);
                assert!(env.is_none());
                assert!(env_vars.is_empty());
                assert!(cwd.is_none());
            }
            other => panic!("unexpected transport {other:?}"),
        }
        assert_eq!(docs.startup_timeout_sec, Some(Duration::from_secs(3)));
        assert_eq!(docs.tool_timeout_sec, Some(Duration::from_secs(5)));
        assert!(docs.enabled);

        let empty = BTreeMap::new();
        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(empty.clone())],
        )?;
        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        assert!(loaded.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn managed_config_wins_over_cli_overrides() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let managed_path = codex_home.path().join("managed_config.toml");

        std::fs::write(
            codex_home.path().join(CONFIG_TOML_FILE),
            "model = \"base\"\n",
        )?;
        std::fs::write(&managed_path, "model = \"managed_config\"\n")?;

        let overrides = LoaderOverrides {
            managed_config_path: Some(managed_path),
            #[cfg(target_os = "macos")]
            managed_preferences_base64: None,
            macos_managed_config_requirements_base64: None,
        };

        let cwd = AbsolutePathBuf::try_from(codex_home.path())?;
        let config_layer_stack = load_config_layers_state(
            codex_home.path(),
            Some(cwd),
            &[("model".to_string(), TomlValue::String("cli".to_string()))],
            overrides,
        )
        .await?;

        let cfg = deserialize_config_toml_with_base(
            config_layer_stack.effective_config(),
            codex_home.path(),
        )
        .map_err(|e| {
            tracing::error!("Failed to deserialize overridden config: {e}");
            e
        })?;

        assert_eq!(cfg.model.as_deref(), Some("managed_config"));
        Ok(())
    }

    #[tokio::test]
    async fn load_global_mcp_servers_accepts_legacy_ms_field() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        std::fs::write(
            &config_path,
            r#"
[mcp_servers]
[mcp_servers.docs]
command = "echo"
startup_timeout_ms = 2500
"#,
        )?;

        let servers = load_global_mcp_servers(codex_home.path()).await?;
        let docs = servers.get("docs").expect("docs entry");
        assert_eq!(docs.startup_timeout_sec, Some(Duration::from_millis(2500)));

        Ok(())
    }

    #[tokio::test]
    async fn load_global_mcp_servers_rejects_inline_bearer_token() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        std::fs::write(
            &config_path,
            r#"
[mcp_servers.docs]
url = "https://example.com/mcp"
bearer_token = "secret"
"#,
        )?;

        let err = load_global_mcp_servers(codex_home.path())
            .await
            .expect_err("bearer_token entries should be rejected");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("bearer_token"));
        assert!(err.to_string().contains("bearer_token_env_var"));

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_serializes_env_sorted() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "docs-server".to_string(),
                    args: vec!["--verbose".to_string()],
                    env: Some(HashMap::from([
                        ("ZIG_VAR".to_string(), "3".to_string()),
                        ("ALPHA_VAR".to_string(), "1".to_string()),
                    ])),
                    env_vars: Vec::new(),
                    cwd: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert_eq!(
            serialized,
            r#"[mcp_servers.docs]
command = "docs-server"
args = ["--verbose"]

[mcp_servers.docs.env]
ALPHA_VAR = "1"
ZIG_VAR = "3"
"#
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::Stdio {
                command,
                args,
                env,
                env_vars,
                cwd,
            } => {
                assert_eq!(command, "docs-server");
                assert_eq!(args, &vec!["--verbose".to_string()]);
                let env = env
                    .as_ref()
                    .expect("env should be preserved for stdio transport");
                assert_eq!(env.get("ALPHA_VAR"), Some(&"1".to_string()));
                assert_eq!(env.get("ZIG_VAR"), Some(&"3".to_string()));
                assert!(env_vars.is_empty());
                assert!(cwd.is_none());
            }
            other => panic!("unexpected transport {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_serializes_env_vars() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "docs-server".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: vec!["ALPHA".to_string(), "BETA".to_string()],
                    cwd: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert!(
            serialized.contains(r#"env_vars = ["ALPHA", "BETA"]"#),
            "serialized config missing env_vars field:\n{serialized}"
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::Stdio { env_vars, .. } => {
                assert_eq!(env_vars, &vec!["ALPHA".to_string(), "BETA".to_string()]);
            }
            other => panic!("unexpected transport {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_serializes_cwd() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let cwd_path = PathBuf::from("/tmp/codex-mcp");
        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "docs-server".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: Some(cwd_path.clone()),
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert!(
            serialized.contains(r#"cwd = "/tmp/codex-mcp""#),
            "serialized config missing cwd field:\n{serialized}"
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::Stdio { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some(Path::new("/tmp/codex-mcp")));
            }
            other => panic!("unexpected transport {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_streamable_http_serializes_bearer_token() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::StreamableHttp {
                    url: "https://example.com/mcp".to_string(),
                    bearer_token_env_var: Some("MCP_TOKEN".to_string()),
                    http_headers: None,
                    env_http_headers: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: Some(Duration::from_secs(2)),
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert_eq!(
            serialized,
            r#"[mcp_servers.docs]
url = "https://example.com/mcp"
bearer_token_env_var = "MCP_TOKEN"
startup_timeout_sec = 2.0
"#
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::StreamableHttp {
                url,
                bearer_token_env_var,
                http_headers,
                env_http_headers,
            } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(bearer_token_env_var.as_deref(), Some("MCP_TOKEN"));
                assert!(http_headers.is_none());
                assert!(env_http_headers.is_none());
            }
            other => panic!("unexpected transport {other:?}"),
        }
        assert_eq!(docs.startup_timeout_sec, Some(Duration::from_secs(2)));

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_streamable_http_serializes_custom_headers() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::StreamableHttp {
                    url: "https://example.com/mcp".to_string(),
                    bearer_token_env_var: Some("MCP_TOKEN".to_string()),
                    http_headers: Some(HashMap::from([("X-Doc".to_string(), "42".to_string())])),
                    env_http_headers: Some(HashMap::from([(
                        "X-Auth".to_string(),
                        "DOCS_AUTH".to_string(),
                    )])),
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: Some(Duration::from_secs(2)),
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);
        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert_eq!(
            serialized,
            r#"[mcp_servers.docs]
url = "https://example.com/mcp"
bearer_token_env_var = "MCP_TOKEN"
startup_timeout_sec = 2.0

[mcp_servers.docs.http_headers]
X-Doc = "42"

[mcp_servers.docs.env_http_headers]
X-Auth = "DOCS_AUTH"
"#
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::StreamableHttp {
                http_headers,
                env_http_headers,
                ..
            } => {
                assert_eq!(
                    http_headers,
                    &Some(HashMap::from([("X-Doc".to_string(), "42".to_string())]))
                );
                assert_eq!(
                    env_http_headers,
                    &Some(HashMap::from([(
                        "X-Auth".to_string(),
                        "DOCS_AUTH".to_string()
                    )]))
                );
            }
            other => panic!("unexpected transport {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_streamable_http_removes_optional_sections() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        let mut servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::StreamableHttp {
                    url: "https://example.com/mcp".to_string(),
                    bearer_token_env_var: Some("MCP_TOKEN".to_string()),
                    http_headers: Some(HashMap::from([("X-Doc".to_string(), "42".to_string())])),
                    env_http_headers: Some(HashMap::from([(
                        "X-Auth".to_string(),
                        "DOCS_AUTH".to_string(),
                    )])),
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: Some(Duration::from_secs(2)),
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;
        let serialized_with_optional = std::fs::read_to_string(&config_path)?;
        assert!(serialized_with_optional.contains("bearer_token_env_var = \"MCP_TOKEN\""));
        assert!(serialized_with_optional.contains("[mcp_servers.docs.http_headers]"));
        assert!(serialized_with_optional.contains("[mcp_servers.docs.env_http_headers]"));

        servers.insert(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::StreamableHttp {
                    url: "https://example.com/mcp".to_string(),
                    bearer_token_env_var: None,
                    http_headers: None,
                    env_http_headers: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        );
        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let serialized = std::fs::read_to_string(&config_path)?;
        assert_eq!(
            serialized,
            r#"[mcp_servers.docs]
url = "https://example.com/mcp"
"#
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::StreamableHttp {
                url,
                bearer_token_env_var,
                http_headers,
                env_http_headers,
            } => {
                assert_eq!(url, "https://example.com/mcp");
                assert!(bearer_token_env_var.is_none());
                assert!(http_headers.is_none());
                assert!(env_http_headers.is_none());
            }
            other => panic!("unexpected transport {other:?}"),
        }

        assert!(docs.startup_timeout_sec.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_streamable_http_isolates_headers_between_servers()
    -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        let servers = BTreeMap::from([
            (
                "docs".to_string(),
                McpServerConfig {
                    transport: McpServerTransportConfig::StreamableHttp {
                        url: "https://example.com/mcp".to_string(),
                        bearer_token_env_var: Some("MCP_TOKEN".to_string()),
                        http_headers: Some(HashMap::from([(
                            "X-Doc".to_string(),
                            "42".to_string(),
                        )])),
                        env_http_headers: Some(HashMap::from([(
                            "X-Auth".to_string(),
                            "DOCS_AUTH".to_string(),
                        )])),
                    },
                    enabled: true,
                    disabled_reason: None,
                    startup_timeout_sec: Some(Duration::from_secs(2)),
                    tool_timeout_sec: None,
                    enabled_tools: None,
                    disabled_tools: None,
                },
            ),
            (
                "logs".to_string(),
                McpServerConfig {
                    transport: McpServerTransportConfig::Stdio {
                        command: "logs-server".to_string(),
                        args: vec!["--follow".to_string()],
                        env: None,
                        env_vars: Vec::new(),
                        cwd: None,
                    },
                    enabled: true,
                    disabled_reason: None,
                    startup_timeout_sec: None,
                    tool_timeout_sec: None,
                    enabled_tools: None,
                    disabled_tools: None,
                },
            ),
        ]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let serialized = std::fs::read_to_string(&config_path)?;
        assert!(
            serialized.contains("[mcp_servers.docs.http_headers]"),
            "serialized config missing docs headers section:\n{serialized}"
        );
        assert!(
            !serialized.contains("[mcp_servers.logs.http_headers]"),
            "serialized config should not add logs headers section:\n{serialized}"
        );
        assert!(
            !serialized.contains("[mcp_servers.logs.env_http_headers]"),
            "serialized config should not add logs env headers section:\n{serialized}"
        );
        assert!(
            !serialized.contains("mcp_servers.logs.bearer_token_env_var"),
            "serialized config should not add bearer token to logs:\n{serialized}"
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        match &docs.transport {
            McpServerTransportConfig::StreamableHttp {
                http_headers,
                env_http_headers,
                ..
            } => {
                assert_eq!(
                    http_headers,
                    &Some(HashMap::from([("X-Doc".to_string(), "42".to_string())]))
                );
                assert_eq!(
                    env_http_headers,
                    &Some(HashMap::from([(
                        "X-Auth".to_string(),
                        "DOCS_AUTH".to_string()
                    )]))
                );
            }
            other => panic!("unexpected transport {other:?}"),
        }
        let logs = loaded.get("logs").expect("logs entry");
        match &logs.transport {
            McpServerTransportConfig::Stdio { env, .. } => {
                assert!(env.is_none());
            }
            other => panic!("unexpected transport {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_serializes_disabled_flag() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "docs-server".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                enabled: false,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert!(
            serialized.contains("enabled = false"),
            "serialized config missing disabled flag:\n{serialized}"
        );

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        assert!(!docs.enabled);

        Ok(())
    }

    #[tokio::test]
    async fn replace_mcp_servers_serializes_tool_filters() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        let servers = BTreeMap::from([(
            "docs".to_string(),
            McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "docs-server".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                enabled: true,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: Some(vec!["allowed".to_string()]),
                disabled_tools: Some(vec!["blocked".to_string()]),
            },
        )]);

        apply_blocking(
            codex_home.path(),
            None,
            &[ConfigEdit::ReplaceMcpServers(servers.clone())],
        )?;

        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let serialized = std::fs::read_to_string(&config_path)?;
        assert!(serialized.contains(r#"enabled_tools = ["allowed"]"#));
        assert!(serialized.contains(r#"disabled_tools = ["blocked"]"#));

        let loaded = load_global_mcp_servers(codex_home.path()).await?;
        let docs = loaded.get("docs").expect("docs entry");
        assert_eq!(
            docs.enabled_tools.as_ref(),
            Some(&vec!["allowed".to_string()])
        );
        assert_eq!(
            docs.disabled_tools.as_ref(),
            Some(&vec!["blocked".to_string()])
        );

        Ok(())
    }

    #[tokio::test]
    async fn set_model_updates_defaults() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        ConfigEditsBuilder::new(codex_home.path())
            .set_model(Some("chosen-model"), Some(ReasoningEffort::High))
            .apply()
            .await?;

        let serialized =
            tokio::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE)).await?;
        let parsed: ConfigToml = toml::from_str(&serialized)?;

        assert_eq!(parsed.model.as_deref(), Some("chosen-model"));
        assert_eq!(parsed.model_reasoning_effort, Some(ReasoningEffort::High));

        Ok(())
    }

    #[tokio::test]
    async fn set_model_overwrites_existing_model() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        tokio::fs::write(
            &config_path,
            r#"
model = "chosen-model"
model_reasoning_effort = "medium"

[profiles.dev]
model = "other-model"
"#,
        )
        .await?;

        ConfigEditsBuilder::new(codex_home.path())
            .set_model(Some("o4-mini"), Some(ReasoningEffort::High))
            .apply()
            .await?;

        let serialized = tokio::fs::read_to_string(config_path).await?;
        let parsed: ConfigToml = toml::from_str(&serialized)?;

        assert_eq!(parsed.model.as_deref(), Some("o4-mini"));
        assert_eq!(parsed.model_reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(
            parsed
                .profiles
                .get("dev")
                .and_then(|profile| profile.model.as_deref()),
            Some("other-model"),
        );

        Ok(())
    }

    #[tokio::test]
    async fn set_model_updates_profile() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;

        ConfigEditsBuilder::new(codex_home.path())
            .with_profile(Some("dev"))
            .set_model(Some("chosen-model"), Some(ReasoningEffort::Medium))
            .apply()
            .await?;

        let serialized =
            tokio::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE)).await?;
        let parsed: ConfigToml = toml::from_str(&serialized)?;
        let profile = parsed
            .profiles
            .get("dev")
            .expect("profile should be created");

        assert_eq!(profile.model.as_deref(), Some("chosen-model"));
        assert_eq!(
            profile.model_reasoning_effort,
            Some(ReasoningEffort::Medium)
        );

        Ok(())
    }

    #[tokio::test]
    async fn set_model_updates_existing_profile() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);

        tokio::fs::write(
            &config_path,
            r#"
[profiles.dev]
model = "server-model-a"
model_reasoning_effort = "medium"

[profiles.prod]
model = "server-model-b"
"#,
        )
        .await?;

        ConfigEditsBuilder::new(codex_home.path())
            .with_profile(Some("dev"))
            .set_model(Some("chosen-model"), Some(ReasoningEffort::Medium))
            .apply()
            .await?;

        let serialized = tokio::fs::read_to_string(config_path).await?;
        let parsed: ConfigToml = toml::from_str(&serialized)?;

        let dev_profile = parsed
            .profiles
            .get("dev")
            .expect("dev profile should survive updates");
        assert_eq!(dev_profile.model.as_deref(), Some("chosen-model"));
        assert_eq!(
            dev_profile.model_reasoning_effort,
            Some(ReasoningEffort::Medium)
        );

        assert_eq!(
            parsed
                .profiles
                .get("prod")
                .and_then(|profile| profile.model.as_deref()),
            Some("server-model-b"),
        );

        Ok(())
    }

    struct PrecedenceTestFixture {
        cwd: TempDir,
        codex_home: TempDir,
        cfg: ConfigToml,
        model_provider_map: HashMap<String, ModelProviderInfo>,
        openwebui_provider: ModelProviderInfo,
        chat_server_provider: ModelProviderInfo,
    }

    impl PrecedenceTestFixture {
        fn cwd(&self) -> PathBuf {
            self.cwd.path().to_path_buf()
        }

        fn codex_home(&self) -> PathBuf {
            self.codex_home.path().to_path_buf()
        }
    }

    #[test]
    fn cli_override_sets_compact_prompt() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let overrides = ConfigOverrides {
            compact_prompt: Some("Use the compact override".to_string()),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            ConfigToml::default(),
            overrides,
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.compact_prompt.as_deref(),
            Some("Use the compact override")
        );

        Ok(())
    }

    #[test]
    fn loads_compact_prompt_from_file() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let workspace = codex_home.path().join("workspace");
        std::fs::create_dir_all(&workspace)?;

        let prompt_path = workspace.join("compact_prompt.txt");
        std::fs::write(&prompt_path, "  summarize differently  ")?;

        let cfg = ConfigToml {
            experimental_compact_prompt_file: Some(AbsolutePathBuf::from_absolute_path(
                prompt_path,
            )?),
            ..Default::default()
        };

        let overrides = ConfigOverrides {
            cwd: Some(workspace),
            ..Default::default()
        };

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            overrides,
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(
            config.compact_prompt.as_deref(),
            Some("summarize differently")
        );

        Ok(())
    }

    fn create_test_fixture() -> std::io::Result<PrecedenceTestFixture> {
        let toml = r#"
model = "top-model"
approval_policy = "untrusted"

# Can be used to determine which profile to use if not specified by
# `ConfigOverrides`.
profile = "chat"

[analytics]
enabled = true

[model_providers.chat-server]
name = "Local chat server"
base_url = "http://127.0.0.1:8080/api/v1"
env_key = "CHAT_SERVER_API_KEY"
wire_api = "chat"
request_max_retries = 4            # retry failed HTTP requests
stream_max_retries = 10            # retry dropped SSE streams
stream_idle_timeout_ms = 300000    # 5m idle timeout

[profiles.deep]
model = "top-model"
model_provider = "openwebui"
approval_policy = "never"
model_reasoning_effort = "high"
model_reasoning_summary = "detailed"

[profiles.chat]
model = "small-chat-model"
model_provider = "chat-server"

[profiles.strict]
model = "top-model"
model_provider = "openwebui"
approval_policy = "on-failure"

[profiles.strict.analytics]
enabled = false

[profiles.detailed]
model = "detailed-model"
model_provider = "openwebui"
approval_policy = "on-failure"
model_reasoning_effort = "high"
model_reasoning_summary = "detailed"
model_verbosity = "high"
"#;

        let cfg: ConfigToml = toml::from_str(toml).expect("TOML deserialization should succeed");

        // Use a temporary directory for the cwd so it does not contain an
        // AGENTS.md file.
        let cwd_temp_dir = TempDir::new().unwrap();
        let cwd = cwd_temp_dir.path().to_path_buf();
        // Make it look like a Git repo so it does not search for AGENTS.md in
        // a parent folder, either.
        std::fs::write(cwd.join(".git"), "gitdir: nowhere")?;

        let codex_home_temp_dir = TempDir::new().unwrap();

        let chat_server_provider = ModelProviderInfo {
            name: "Local chat server".to_string(),
            base_url: Some("http://127.0.0.1:8080/api/v1".to_string()),
            env_key: Some("CHAT_SERVER_API_KEY".to_string()),
            wire_api: crate::WireApi::Chat,
            env_key_instructions: None,
            experimental_bearer_token: None,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_max_retries: Some(10),
            stream_idle_timeout_ms: Some(300_000),
        };
        let model_provider_map = {
            let mut model_provider_map = built_in_model_providers();
            model_provider_map.insert("chat-server".to_string(), chat_server_provider.clone());
            model_provider_map
        };

        let openwebui_provider = model_provider_map
            .get(OPENWEBUI_PROVIDER_ID)
            .expect("openwebui provider should exist")
            .clone();

        Ok(PrecedenceTestFixture {
            cwd: cwd_temp_dir,
            codex_home: codex_home_temp_dir,
            cfg,
            model_provider_map,
            openwebui_provider,
            chat_server_provider,
        })
    }

    /// Users can specify config values at multiple levels that have the
    /// following precedence:
    ///
    /// 1. custom command-line argument, e.g. `--model my-model`
    /// 2. as part of a profile, where the `--profile` is specified via a CLI
    ///    (or in the config file itself)
    /// 3. as an entry in `config.toml`, e.g. `model = "my-model"`
    /// 4. the default value for a required field defined in code, e.g.,
    ///    owl's built-in model provider (the user's Open WebUI server)
    ///
    /// Note that profiles are the recommended way to specify a group of
    /// configuration options together.
    #[test]
    fn test_precedence_fixture_with_deep_profile() -> std::io::Result<()> {
        let fixture = create_test_fixture()?;

        let deep_profile_overrides = ConfigOverrides {
            config_profile: Some("deep".to_string()),
            cwd: Some(fixture.cwd()),
            ..Default::default()
        };
        let deep_profile_config: Config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            deep_profile_overrides,
            fixture.codex_home(),
        )?;
        assert_eq!(
            Config {
                model: Some("top-model".to_string()),
                review_model: None,
                model_context_window: None,
                model_auto_compact_token_limit: None,
                model_provider_id: OPENWEBUI_PROVIDER_ID.to_string(),
                model_provider: fixture.openwebui_provider.clone(),
                approval_policy: Constrained::allow_any(AskForApproval::Never),
                sandbox_policy: Constrained::allow_any(SandboxPolicy::new_read_only_policy()),
                did_user_set_custom_approval_policy_or_sandbox_mode: true,
                forced_auto_mode_downgraded_on_windows: false,
                shell_environment_policy: ShellEnvironmentPolicy::default(),
                user_instructions: None,
                notify: None,
                cwd: fixture.cwd(),
                cli_auth_credentials_store_mode: Default::default(),
                mcp_servers: Constrained::allow_any(HashMap::new()),
                mcp_oauth_credentials_store_mode: Default::default(),
                mcp_oauth_callback_port: None,
                model_providers: fixture.model_provider_map.clone(),
                project_doc_max_bytes: PROJECT_DOC_MAX_BYTES,
                project_doc_fallback_filenames: Vec::new(),
                tool_output_token_limit: None,
                agent_max_threads: DEFAULT_AGENT_MAX_THREADS,
                codex_home: fixture.codex_home(),
                config_layer_stack: Default::default(),
                history: History::default(),
                ephemeral: false,
                file_opener: UriBasedFileOpener::VsCode,
                codex_linux_sandbox_exe: None,
                hide_agent_reasoning: false,
                show_raw_agent_reasoning: false,
                model_reasoning_effort: Some(ReasoningEffort::High),
                model_reasoning_summary: ReasoningSummary::Detailed,
                model_supports_reasoning_summaries: None,
                model_verbosity: None,
                model_personality: None,
                base_instructions: None,
                developer_instructions: None,
                compact_prompt: None,
                include_apply_patch_tool: false,
                web_search_mode: None,
                web_search_backend: None,
                use_experimental_unified_exec_tool: false,
                ghost_snapshot: GhostSnapshotConfig::default(),
                features: Features::with_defaults(),
                active_profile: Some("deep".to_string()),
                active_project: ProjectConfig { trust_level: None },
                windows_wsl_setup_acknowledged: false,
                notices: Default::default(),
                disable_paste_burst: false,
                tui_notifications: Default::default(),
                animations: true,
                show_tooltips: true,
                experimental_mode: None,
                analytics_enabled: Some(true),
                feedback_enabled: true,
                tui_alternate_screen: AltScreenMode::Auto,
                otel: OtelConfig::default(),
            },
            deep_profile_config
        );
        Ok(())
    }

    #[test]
    fn test_precedence_fixture_with_chat_profile() -> std::io::Result<()> {
        let fixture = create_test_fixture()?;

        let chat_profile_overrides = ConfigOverrides {
            config_profile: Some("chat".to_string()),
            cwd: Some(fixture.cwd()),
            ..Default::default()
        };
        let chat_profile_config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            chat_profile_overrides,
            fixture.codex_home(),
        )?;
        let expected_chat_profile_config = Config {
            model: Some("small-chat-model".to_string()),
            review_model: None,
            model_context_window: None,
            model_auto_compact_token_limit: None,
            model_provider_id: "chat-server".to_string(),
            model_provider: fixture.chat_server_provider.clone(),
            approval_policy: Constrained::allow_any(AskForApproval::UnlessTrusted),
            sandbox_policy: Constrained::allow_any(SandboxPolicy::new_read_only_policy()),
            did_user_set_custom_approval_policy_or_sandbox_mode: true,
            forced_auto_mode_downgraded_on_windows: false,
            shell_environment_policy: ShellEnvironmentPolicy::default(),
            user_instructions: None,
            notify: None,
            cwd: fixture.cwd(),
            cli_auth_credentials_store_mode: Default::default(),
            mcp_servers: Constrained::allow_any(HashMap::new()),
            mcp_oauth_credentials_store_mode: Default::default(),
            mcp_oauth_callback_port: None,
            model_providers: fixture.model_provider_map.clone(),
            project_doc_max_bytes: PROJECT_DOC_MAX_BYTES,
            project_doc_fallback_filenames: Vec::new(),
            tool_output_token_limit: None,
            agent_max_threads: DEFAULT_AGENT_MAX_THREADS,
            codex_home: fixture.codex_home(),
            config_layer_stack: Default::default(),
            history: History::default(),
            ephemeral: false,
            file_opener: UriBasedFileOpener::VsCode,
            codex_linux_sandbox_exe: None,
            hide_agent_reasoning: false,
            show_raw_agent_reasoning: false,
            model_reasoning_effort: None,
            model_reasoning_summary: ReasoningSummary::default(),
            model_supports_reasoning_summaries: None,
            model_verbosity: None,
            model_personality: None,
            base_instructions: None,
            developer_instructions: None,
            compact_prompt: None,
            include_apply_patch_tool: false,
            web_search_mode: None,
            web_search_backend: None,
            use_experimental_unified_exec_tool: false,
            ghost_snapshot: GhostSnapshotConfig::default(),
            features: Features::with_defaults(),
            active_profile: Some("chat".to_string()),
            active_project: ProjectConfig { trust_level: None },
            windows_wsl_setup_acknowledged: false,
            notices: Default::default(),
            disable_paste_burst: false,
            tui_notifications: Default::default(),
            animations: true,
            show_tooltips: true,
            experimental_mode: None,
            analytics_enabled: Some(true),
            feedback_enabled: true,
            tui_alternate_screen: AltScreenMode::Auto,
            otel: OtelConfig::default(),
        };

        assert_eq!(expected_chat_profile_config, chat_profile_config);

        // Verify that loading without specifying a profile in ConfigOverrides
        // uses the default profile from the config file (which is "chat").
        let default_profile_overrides = ConfigOverrides {
            cwd: Some(fixture.cwd()),
            ..Default::default()
        };

        let default_profile_config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            default_profile_overrides,
            fixture.codex_home(),
        )?;

        assert_eq!(expected_chat_profile_config, default_profile_config);
        Ok(())
    }

    #[test]
    fn test_precedence_fixture_with_strict_profile() -> std::io::Result<()> {
        let fixture = create_test_fixture()?;

        let strict_profile_overrides = ConfigOverrides {
            config_profile: Some("strict".to_string()),
            cwd: Some(fixture.cwd()),
            ..Default::default()
        };
        let strict_profile_config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            strict_profile_overrides,
            fixture.codex_home(),
        )?;
        let expected_strict_profile_config = Config {
            model: Some("top-model".to_string()),
            review_model: None,
            model_context_window: None,
            model_auto_compact_token_limit: None,
            model_provider_id: OPENWEBUI_PROVIDER_ID.to_string(),
            model_provider: fixture.openwebui_provider.clone(),
            approval_policy: Constrained::allow_any(AskForApproval::OnFailure),
            sandbox_policy: Constrained::allow_any(SandboxPolicy::new_read_only_policy()),
            did_user_set_custom_approval_policy_or_sandbox_mode: true,
            forced_auto_mode_downgraded_on_windows: false,
            shell_environment_policy: ShellEnvironmentPolicy::default(),
            user_instructions: None,
            notify: None,
            cwd: fixture.cwd(),
            cli_auth_credentials_store_mode: Default::default(),
            mcp_servers: Constrained::allow_any(HashMap::new()),
            mcp_oauth_credentials_store_mode: Default::default(),
            mcp_oauth_callback_port: None,
            model_providers: fixture.model_provider_map.clone(),
            project_doc_max_bytes: PROJECT_DOC_MAX_BYTES,
            project_doc_fallback_filenames: Vec::new(),
            tool_output_token_limit: None,
            agent_max_threads: DEFAULT_AGENT_MAX_THREADS,
            codex_home: fixture.codex_home(),
            config_layer_stack: Default::default(),
            history: History::default(),
            ephemeral: false,
            file_opener: UriBasedFileOpener::VsCode,
            codex_linux_sandbox_exe: None,
            hide_agent_reasoning: false,
            show_raw_agent_reasoning: false,
            model_reasoning_effort: None,
            model_reasoning_summary: ReasoningSummary::default(),
            model_supports_reasoning_summaries: None,
            model_verbosity: None,
            model_personality: None,
            base_instructions: None,
            developer_instructions: None,
            compact_prompt: None,
            include_apply_patch_tool: false,
            web_search_mode: None,
            web_search_backend: None,
            use_experimental_unified_exec_tool: false,
            ghost_snapshot: GhostSnapshotConfig::default(),
            features: Features::with_defaults(),
            active_profile: Some("strict".to_string()),
            active_project: ProjectConfig { trust_level: None },
            windows_wsl_setup_acknowledged: false,
            notices: Default::default(),
            disable_paste_burst: false,
            tui_notifications: Default::default(),
            animations: true,
            show_tooltips: true,
            experimental_mode: None,
            analytics_enabled: Some(false),
            feedback_enabled: true,
            tui_alternate_screen: AltScreenMode::Auto,
            otel: OtelConfig::default(),
        };

        assert_eq!(expected_strict_profile_config, strict_profile_config);

        Ok(())
    }

    #[test]
    fn test_precedence_fixture_with_detailed_profile() -> std::io::Result<()> {
        let fixture = create_test_fixture()?;

        let detailed_profile_overrides = ConfigOverrides {
            config_profile: Some("detailed".to_string()),
            cwd: Some(fixture.cwd()),
            ..Default::default()
        };
        let detailed_profile_config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            detailed_profile_overrides,
            fixture.codex_home(),
        )?;
        let expected_detailed_profile_config = Config {
            model: Some("detailed-model".to_string()),
            review_model: None,
            model_context_window: None,
            model_auto_compact_token_limit: None,
            model_provider_id: OPENWEBUI_PROVIDER_ID.to_string(),
            model_provider: fixture.openwebui_provider.clone(),
            approval_policy: Constrained::allow_any(AskForApproval::OnFailure),
            sandbox_policy: Constrained::allow_any(SandboxPolicy::new_read_only_policy()),
            did_user_set_custom_approval_policy_or_sandbox_mode: true,
            forced_auto_mode_downgraded_on_windows: false,
            shell_environment_policy: ShellEnvironmentPolicy::default(),
            user_instructions: None,
            notify: None,
            cwd: fixture.cwd(),
            cli_auth_credentials_store_mode: Default::default(),
            mcp_servers: Constrained::allow_any(HashMap::new()),
            mcp_oauth_credentials_store_mode: Default::default(),
            mcp_oauth_callback_port: None,
            model_providers: fixture.model_provider_map.clone(),
            project_doc_max_bytes: PROJECT_DOC_MAX_BYTES,
            project_doc_fallback_filenames: Vec::new(),
            tool_output_token_limit: None,
            agent_max_threads: DEFAULT_AGENT_MAX_THREADS,
            codex_home: fixture.codex_home(),
            config_layer_stack: Default::default(),
            history: History::default(),
            ephemeral: false,
            file_opener: UriBasedFileOpener::VsCode,
            codex_linux_sandbox_exe: None,
            hide_agent_reasoning: false,
            show_raw_agent_reasoning: false,
            model_reasoning_effort: Some(ReasoningEffort::High),
            model_reasoning_summary: ReasoningSummary::Detailed,
            model_supports_reasoning_summaries: None,
            model_verbosity: Some(Verbosity::High),
            model_personality: None,
            base_instructions: None,
            developer_instructions: None,
            compact_prompt: None,
            include_apply_patch_tool: false,
            web_search_mode: None,
            web_search_backend: None,
            use_experimental_unified_exec_tool: false,
            ghost_snapshot: GhostSnapshotConfig::default(),
            features: Features::with_defaults(),
            active_profile: Some("detailed".to_string()),
            active_project: ProjectConfig { trust_level: None },
            windows_wsl_setup_acknowledged: false,
            notices: Default::default(),
            disable_paste_burst: false,
            tui_notifications: Default::default(),
            animations: true,
            show_tooltips: true,
            experimental_mode: None,
            analytics_enabled: Some(true),
            feedback_enabled: true,
            tui_alternate_screen: AltScreenMode::Auto,
            otel: OtelConfig::default(),
        };

        assert_eq!(expected_detailed_profile_config, detailed_profile_config);

        Ok(())
    }

    #[test]
    fn test_did_user_set_custom_approval_policy_or_sandbox_mode_defaults_no() -> anyhow::Result<()>
    {
        let fixture = create_test_fixture()?;

        let config = Config::load_from_base_config_with_overrides(
            fixture.cfg.clone(),
            ConfigOverrides {
                ..Default::default()
            },
            fixture.codex_home(),
        )?;

        assert!(config.did_user_set_custom_approval_policy_or_sandbox_mode);

        Ok(())
    }

    #[test]
    fn test_set_project_trusted_writes_explicit_tables() -> anyhow::Result<()> {
        let project_dir = Path::new("/some/path");
        let mut doc = DocumentMut::new();

        set_project_trust_level_inner(&mut doc, project_dir, TrustLevel::Trusted)?;

        let contents = doc.to_string();

        let raw_path = project_dir.to_string_lossy();
        let path_str = if raw_path.contains('\\') {
            format!("'{raw_path}'")
        } else {
            format!("\"{raw_path}\"")
        };
        let expected = format!(
            r#"[projects.{path_str}]
trust_level = "trusted"
"#
        );
        assert_eq!(contents, expected);

        Ok(())
    }

    #[test]
    fn test_set_project_trusted_converts_inline_to_explicit() -> anyhow::Result<()> {
        let project_dir = Path::new("/some/path");

        // Seed config.toml with an inline project entry under [projects]
        let raw_path = project_dir.to_string_lossy();
        let path_str = if raw_path.contains('\\') {
            format!("'{raw_path}'")
        } else {
            format!("\"{raw_path}\"")
        };
        // Use a quoted key so backslashes don't require escaping on Windows
        let initial = format!(
            r#"[projects]
{path_str} = {{ trust_level = "untrusted" }}
"#
        );
        let mut doc = initial.parse::<DocumentMut>()?;

        // Run the function; it should convert to explicit tables and set trusted
        set_project_trust_level_inner(&mut doc, project_dir, TrustLevel::Trusted)?;

        let contents = doc.to_string();

        // Assert exact output after conversion to explicit table
        let expected = format!(
            r#"[projects]

[projects.{path_str}]
trust_level = "trusted"
"#
        );
        assert_eq!(contents, expected);

        Ok(())
    }

    #[test]
    fn test_set_project_trusted_migrates_top_level_inline_projects_preserving_entries()
    -> anyhow::Result<()> {
        let initial = r#"toplevel = "baz"
projects = { "/Users/mbolin/code/codex4" = { trust_level = "trusted", foo = "bar" } , "/Users/mbolin/code/codex3" = { trust_level = "trusted" } }
model = "foo""#;
        let mut doc = initial.parse::<DocumentMut>()?;

        // Approve a new directory
        let new_project = Path::new("/Users/mbolin/code/codex2");
        set_project_trust_level_inner(&mut doc, new_project, TrustLevel::Trusted)?;

        let contents = doc.to_string();

        // Since we created the [projects] table as part of migration, it is kept implicit.
        // Expect explicit per-project tables, preserving prior entries and appending the new one.
        let expected = r#"toplevel = "baz"
model = "foo"

[projects."/Users/mbolin/code/codex4"]
trust_level = "trusted"
foo = "bar"

[projects."/Users/mbolin/code/codex3"]
trust_level = "trusted"

[projects."/Users/mbolin/code/codex2"]
trust_level = "trusted"
"#;
        assert_eq!(contents, expected);

        Ok(())
    }

    #[test]
    fn test_set_default_oss_provider() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let codex_home = temp_dir.path();
        let config_path = codex_home.join(CONFIG_TOML_FILE);

        // Test setting valid provider on empty config
        set_default_oss_provider(codex_home, OLLAMA_OSS_PROVIDER_ID)?;
        let content = std::fs::read_to_string(&config_path)?;
        assert!(content.contains("oss_provider = \"ollama\""));

        // Test updating existing config
        std::fs::write(&config_path, "model = \"gpt-4\"\n")?;
        set_default_oss_provider(codex_home, LMSTUDIO_OSS_PROVIDER_ID)?;
        let content = std::fs::read_to_string(&config_path)?;
        assert!(content.contains("oss_provider = \"lmstudio\""));
        assert!(content.contains("model = \"gpt-4\""));

        // Test overwriting existing oss_provider
        set_default_oss_provider(codex_home, OLLAMA_OSS_PROVIDER_ID)?;
        let content = std::fs::read_to_string(&config_path)?;
        assert!(content.contains("oss_provider = \"ollama\""));
        assert!(!content.contains("oss_provider = \"lmstudio\""));

        // Test invalid provider
        let result = set_default_oss_provider(codex_home, "invalid_provider");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("Invalid OSS provider"));
        assert!(error.to_string().contains("invalid_provider"));

        Ok(())
    }

    #[test]
    fn test_untrusted_project_gets_workspace_write_sandbox() -> anyhow::Result<()> {
        let config_with_untrusted = r#"
[projects."/tmp/test"]
trust_level = "untrusted"
"#;

        let cfg = toml::from_str::<ConfigToml>(config_with_untrusted)
            .expect("TOML deserialization should succeed");

        let resolution = cfg.derive_sandbox_policy(None, None, &PathBuf::from("/tmp/test"));

        // Verify that untrusted projects get WorkspaceWrite (or ReadOnly on Windows due to downgrade)
        if cfg!(target_os = "windows") {
            assert!(
                matches!(resolution.policy, SandboxPolicy::ReadOnly),
                "Expected ReadOnly on Windows, got {:?}",
                resolution.policy
            );
        } else {
            assert!(
                matches!(resolution.policy, SandboxPolicy::WorkspaceWrite { .. }),
                "Expected WorkspaceWrite for untrusted project, got {:?}",
                resolution.policy
            );
        }

        Ok(())
    }

    #[test]
    fn test_resolve_oss_provider_explicit_override() {
        let config_toml = ConfigToml::default();
        let result = resolve_oss_provider(Some("custom-provider"), &config_toml, None);
        assert_eq!(result, Some("custom-provider".to_string()));
    }

    #[test]
    fn test_resolve_oss_provider_from_profile() {
        let mut profiles = std::collections::HashMap::new();
        let profile = ConfigProfile {
            oss_provider: Some("profile-provider".to_string()),
            ..Default::default()
        };
        profiles.insert("test-profile".to_string(), profile);
        let config_toml = ConfigToml {
            profiles,
            ..Default::default()
        };

        let result = resolve_oss_provider(None, &config_toml, Some("test-profile".to_string()));
        assert_eq!(result, Some("profile-provider".to_string()));
    }

    #[test]
    fn test_resolve_oss_provider_from_global_config() {
        let config_toml = ConfigToml {
            oss_provider: Some("global-provider".to_string()),
            ..Default::default()
        };

        let result = resolve_oss_provider(None, &config_toml, None);
        assert_eq!(result, Some("global-provider".to_string()));
    }

    #[test]
    fn test_resolve_oss_provider_profile_fallback_to_global() {
        let mut profiles = std::collections::HashMap::new();
        let profile = ConfigProfile::default(); // No oss_provider set
        profiles.insert("test-profile".to_string(), profile);
        let config_toml = ConfigToml {
            oss_provider: Some("global-provider".to_string()),
            profiles,
            ..Default::default()
        };

        let result = resolve_oss_provider(None, &config_toml, Some("test-profile".to_string()));
        assert_eq!(result, Some("global-provider".to_string()));
    }

    #[test]
    fn test_resolve_oss_provider_none_when_not_configured() {
        let config_toml = ConfigToml::default();
        let result = resolve_oss_provider(None, &config_toml, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_oss_provider_explicit_overrides_all() {
        let mut profiles = std::collections::HashMap::new();
        let profile = ConfigProfile {
            oss_provider: Some("profile-provider".to_string()),
            ..Default::default()
        };
        profiles.insert("test-profile".to_string(), profile);
        let config_toml = ConfigToml {
            oss_provider: Some("global-provider".to_string()),
            profiles,
            ..Default::default()
        };

        let result = resolve_oss_provider(
            Some("explicit-provider"),
            &config_toml,
            Some("test-profile".to_string()),
        );
        assert_eq!(result, Some("explicit-provider".to_string()));
    }

    #[test]
    fn config_toml_deserializes_mcp_oauth_callback_port() {
        let toml = r#"mcp_oauth_callback_port = 4321"#;
        let cfg: ConfigToml =
            toml::from_str(toml).expect("TOML deserialization should succeed for callback port");
        assert_eq!(cfg.mcp_oauth_callback_port, Some(4321));
    }

    #[test]
    fn config_loads_mcp_oauth_callback_port_from_toml() -> std::io::Result<()> {
        let codex_home = TempDir::new()?;
        let toml = r#"
model = "gpt-5.1"
mcp_oauth_callback_port = 5678
"#;
        let cfg: ConfigToml =
            toml::from_str(toml).expect("TOML deserialization should succeed for callback port");

        let config = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.path().to_path_buf(),
        )?;

        assert_eq!(config.mcp_oauth_callback_port, Some(5678));
        Ok(())
    }

    #[test]
    fn test_untrusted_project_gets_unless_trusted_approval_policy() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let test_project_dir = TempDir::new()?;
        let test_path = test_project_dir.path();

        let config = Config::load_from_base_config_with_overrides(
            ConfigToml {
                projects: Some(HashMap::from([(
                    test_path.to_string_lossy().to_string(),
                    ProjectConfig {
                        trust_level: Some(TrustLevel::Untrusted),
                    },
                )])),
                ..Default::default()
            },
            ConfigOverrides {
                cwd: Some(test_path.to_path_buf()),
                ..Default::default()
            },
            codex_home.path().to_path_buf(),
        )?;

        // Verify that untrusted projects get UnlessTrusted approval policy
        assert_eq!(
            config.approval_policy.value(),
            AskForApproval::UnlessTrusted,
            "Expected UnlessTrusted approval policy for untrusted project"
        );

        // Verify that untrusted projects still get WorkspaceWrite sandbox (or ReadOnly on Windows)
        if cfg!(target_os = "windows") {
            assert!(
                matches!(config.sandbox_policy.get(), SandboxPolicy::ReadOnly),
                "Expected ReadOnly on Windows"
            );
        } else {
            assert!(
                matches!(
                    config.sandbox_policy.get(),
                    SandboxPolicy::WorkspaceWrite { .. }
                ),
                "Expected WorkspaceWrite sandbox for untrusted project"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod notifications_tests {
    use crate::config::types::Notifications;
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct TuiTomlTest {
        notifications: Notifications,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct RootTomlTest {
        tui: TuiTomlTest,
    }

    #[test]
    fn test_tui_notifications_true() {
        let toml = r#"
            [tui]
            notifications = true
        "#;
        let parsed: RootTomlTest = toml::from_str(toml).expect("deserialize notifications=true");
        assert_matches!(parsed.tui.notifications, Notifications::Enabled(true));
    }

    #[test]
    fn test_tui_notifications_custom_array() {
        let toml = r#"
            [tui]
            notifications = ["foo"]
        "#;
        let parsed: RootTomlTest =
            toml::from_str(toml).expect("deserialize notifications=[\"foo\"]");
        assert_matches!(
            parsed.tui.notifications,
            Notifications::Custom(ref v) if v == &vec!["foo".to_string()]
        );
    }
}
