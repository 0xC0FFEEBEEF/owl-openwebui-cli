use super::cache::ModelsCacheManager;
use super::cache::ModelsCacheSource;
use crate::api_bridge::map_api_error;
use crate::config::Config;
use crate::default_client::build_reqwest_client;
use crate::error::CodexErr;
use crate::error::Result as CoreResult;
use crate::features::Feature;
use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::OPENWEBUI_PROVIDER_ID;
use crate::model_provider_info::WireApi;
use crate::models_manager::collaboration_mode_presets::builtin_collaboration_mode_presets;
use crate::models_manager::model_info;
use codex_api::ReqwestTransport;
use codex_client::HttpTransport;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelVisibility;
use http::Method;
use http::header::AUTHORIZATION;
use http::header::ETAG;
use http::header::HeaderValue;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::TryLockError;
use tokio::time::timeout;
use tracing::error;

const MODEL_CACHE_FILE: &str = "models_cache.json";
const DEFAULT_MODEL_CACHE_TTL: Duration = Duration::from_secs(300);
const MODELS_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);

/// Strategy for refreshing available models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshStrategy {
    /// Always fetch from the network, ignoring cache.
    Online,
    /// Only use cached data, never fetch from the network.
    Offline,
    /// Use cache if available and fresh, otherwise fetch from the network.
    OnlineIfUncached,
}

/// Parse the model listing served by `{base_url}/models` into model metadata usable
/// by the picker and default-model selection.
///
/// owl asks the server the user configured, so it accepts what such a server answers
/// with:
/// - `{"data":[...]}` or a bare array: the OpenAI-shaped listing Open WebUI serves,
///   where `id` is the model name and `name` is what to show the user. Everything else
///   comes from owl's own defaults for a model it does not know.
/// - `{"models":[...]}`: the complete model metadata a self-hosted gateway can serve
///   (`slug`, `visibility`, `priority`, reasoning levels, truncation policy, ...).
///   A partial entry of this shape is not used: owl takes model metadata whole or not
///   at all.
fn parse_server_model_listing(body: &[u8]) -> Vec<ModelInfo> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let items: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Object(map) => ["data", "models"]
            .iter()
            .filter_map(|key| map.get(*key))
            .find_map(serde_json::Value::as_array)
            .map(|arr| arr.iter().collect())
            .unwrap_or_default(),
        serde_json::Value::Array(arr) => arr.iter().collect(),
        _ => Vec::new(),
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                let id = id.trim();
                if id.is_empty() {
                    return None;
                }
                let mut info = model_info::find_model_info_for_slug(id);
                info.slug = id.to_string();
                info.display_name = item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(id)
                    .to_string();
                info.visibility = ModelVisibility::List;
                info.supported_in_api = true;
                info.priority = index as i32;
                return Some(info);
            }
            let info = serde_json::from_value::<ModelInfo>((*item).clone()).ok()?;
            if info.slug.trim().is_empty() {
                return None;
            }
            Some(info)
        })
        .collect()
}

/// Coordinates remote model discovery plus cached metadata on disk.
#[derive(Debug)]
pub struct ModelsManager {
    remote_models: RwLock<Vec<ModelInfo>>,
    /// Which provider and base URL `remote_models` came from. `None` when the
    /// list did not come from the server owl asked, which no server-listed
    /// provider may borrow.
    remote_source: RwLock<Option<ModelsCacheSource>>,
    /// Why the selected server's model list is unavailable, for the UI to show.
    list_error: RwLock<Option<String>>,
    etag: RwLock<Option<String>>,
    cache_manager: ModelsCacheManager,
    /// Test-only provider override. Ordinary runs resolve the provider from
    /// `Config`, so the Open WebUI path sees the user's own server instead of
    /// a hardcoded OpenAI provider.
    provider: Option<ModelProviderInfo>,
}

impl ModelsManager {
    /// Provider this manager should use: the injected test override when
    /// present, otherwise the provider resolved from `Config`. Returns an owned
    /// copy so the returned value isn't tied to `self`'s lifetime.
    fn effective_provider(&self, config: &Config) -> ModelProviderInfo {
        self.provider
            .clone()
            .unwrap_or_else(|| config.model_provider.clone())
    }

    /// True when this manager is operating against the built-in Open WebUI
    /// provider, whose model list comes from the user's own server instead of
    /// OpenAI's catalog. Providers that reuse the built-in id but customize
    /// `wire_api` behave like regular providers.
    ///
    /// Public because the TUI needs it to decide whether the model a fresh session
    /// starts with has to be looked up on the server before it can be chosen.
    pub fn openwebui_provider_selected(&self, config: &Config) -> bool {
        config.model_provider_id == OPENWEBUI_PROVIDER_ID
            && self.effective_provider(config).wire_api == WireApi::Chat
    }

    /// True when the selected provider is asked for its own model list at
    /// `{base_url}/models`.
    ///
    /// owl ships no model list of its own, so any provider with a base URL is asked
    /// which models it serves, and only those models may appear in the picker or start
    /// a session. A provider without a base URL cannot be asked, so it has none.
    pub fn server_model_list_selected(&self, config: &Config) -> bool {
        self.effective_provider(config).base_url.is_some()
    }

    /// Identity of the model list a server-listed provider is allowed to use.
    fn models_cache_source(&self, config: &Config) -> ModelsCacheSource {
        let provider = self.effective_provider(config);
        ModelsCacheSource {
            provider_id: config.model_provider_id.clone(),
            base_url: provider.base_url.unwrap_or_default(),
        }
    }

    /// Why the selected server's model list is currently unavailable, if it is.
    ///
    /// The TUI and `owl exec` show this instead of quietly using models the
    /// user's server has never heard of.
    pub async fn model_list_error(&self) -> Option<String> {
        self.list_error.read().await.clone()
    }

    /// True when the list currently held in `remote_models` came from the
    /// selected server, so a server-listed provider may use it.
    async fn remote_models_match_server(&self, config: &Config) -> bool {
        let expected = self.models_cache_source(config);
        self.remote_source
            .read()
            .await
            .as_ref()
            .is_some_and(|source| *source == expected)
    }

    /// Drop the current model list so a failed or absent server list cannot be
    /// answered with models the user's server never mentioned.
    async fn clear_server_models(&self) {
        *self.remote_models.write().await = Vec::new();
        *self.remote_source.write().await = None;
        *self.etag.write().await = None;
    }

    ///
    /// Uses `codex_home` to store cached model metadata.
    ///
    /// owl ships no model list of its own, so the manager starts empty until the
    /// selected provider's list has been fetched or loaded from that cache.
    /// Construct a manager whose model list is cached under `codex_home`.
    pub fn new(codex_home: PathBuf) -> Self {
        let cache_path = codex_home.join(MODEL_CACHE_FILE);
        let cache_manager = ModelsCacheManager::new(cache_path, DEFAULT_MODEL_CACHE_TTL);
        Self {
            remote_models: RwLock::new(Vec::new()),
            remote_source: RwLock::new(None),
            list_error: RwLock::new(None),
            etag: RwLock::new(None),
            cache_manager,
            provider: None,
        }
    }

    /// List all available models, refreshing according to the specified strategy.
    ///
    /// Returns model presets sorted by priority and filtered by auth mode and visibility.
    pub async fn list_models(
        &self,
        config: &Config,
        refresh_strategy: RefreshStrategy,
    ) -> Vec<ModelPreset> {
        if let Err(err) = self
            .refresh_available_models(config, refresh_strategy)
            .await
        {
            error!("failed to refresh available models: {err}");
        }
        let remote_models = self.get_remote_models(config).await;
        self.build_available_models(remote_models)
    }

    /// List collaboration mode presets.
    ///
    /// Returns a static set of presets seeded with the configured model.
    pub fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        builtin_collaboration_mode_presets()
    }

    /// Attempt to list models without blocking, using the current cached state.
    ///
    /// Returns an error if the internal lock cannot be acquired.
    pub fn try_list_models(&self, config: &Config) -> Result<Vec<ModelPreset>, TryLockError> {
        let remote_models = self.try_get_remote_models(config)?;
        Ok(self.build_available_models(remote_models))
    }

    // todo(aibrahim): should be visible to core only and sent on session_configured event
    /// Get the model identifier to use, refreshing according to the specified strategy.
    ///
    /// If `model` is provided, returns it directly. Otherwise selects the default based on
    /// auth mode and available models.
    pub async fn get_default_model(
        &self,
        model: &Option<String>,
        config: &Config,
        refresh_strategy: RefreshStrategy,
    ) -> String {
        if let Some(model) = model.as_ref() {
            return model.to_string();
        }
        if let Err(err) = self
            .refresh_available_models(config, refresh_strategy)
            .await
        {
            error!("failed to refresh available models: {err}");
        }
        let remote_models = self.get_remote_models(config).await;
        let available = self.build_available_models(remote_models);
        available
            .iter()
            .find(|model| model.is_default)
            .or_else(|| available.first())
            .map(|model| model.model.clone())
            .unwrap_or_default()
    }

    // todo(aibrahim): look if we can tighten it to pub(crate)
    /// Look up model metadata, applying remote overrides and config adjustments.
    pub async fn get_model_info(&self, model: &str, config: &Config) -> ModelInfo {
        let remote = self
            .get_remote_models(config)
            .await
            .into_iter()
            .find(|m| m.slug == model);
        let model = if let Some(remote) = remote {
            remote
        } else {
            model_info::find_model_info_for_slug(model)
        };
        model_info::with_config_overrides(model, config)
    }

    /// Refresh models if the provided ETag differs from the cached ETag.
    ///
    /// Uses `Online` strategy to fetch latest models when ETags differ.
    pub(crate) async fn refresh_if_new_etag(&self, etag: String, config: &Config) {
        let current_etag = self.get_etag().await;
        if current_etag.clone().is_some() && current_etag.as_deref() == Some(etag.as_str()) {
            if let Err(err) = self.cache_manager.renew_cache_ttl().await {
                error!("failed to renew cache TTL: {err}");
            }
            return;
        }
        if let Err(err) = self
            .refresh_available_models(config, RefreshStrategy::Online)
            .await
        {
            error!("failed to refresh available models: {err}");
        }
    }

    /// Refresh available models according to the specified strategy.
    ///
    /// The only model list owl knows is the one the selected provider's server serves,
    /// so every strategy either loads that list (from cache or over the network) or
    /// leaves the list empty.
    async fn refresh_available_models(
        &self,
        config: &Config,
        refresh_strategy: RefreshStrategy,
    ) -> CoreResult<()> {
        if !config.features.enabled(Feature::RemoteModels)
            || !self.server_model_list_selected(config)
        {
            // Either the user turned the server model list off with
            // `remote_models = false`, or the provider has no URL to ask.
            self.clear_server_models().await;
            return Ok(());
        }
        self.refresh_server_listed_models(config, refresh_strategy)
            .await
    }

    /// Refresh the model list from the selected provider's `{base_url}/models`
    /// endpoint.
    ///
    /// Fetched models *replace* the model list outright, so the picker and
    /// default-model selection only ever show models the user's own server
    /// actually serves.
    ///
    /// A cache that is still within its TTL answers `Offline` and
    /// `OnlineIfUncached`, so owl does not ask the user's server for its models
    /// on every turn; `Online` always refetches.
    async fn refresh_server_listed_models(
        &self,
        config: &Config,
        refresh_strategy: RefreshStrategy,
    ) -> CoreResult<()> {
        let source = self.models_cache_source(config);
        // `Online` means something told us the list changed (a new model-list ETag
        // from the server), so a cache that is still inside its TTL is not an
        // answer here; ask the server again.
        let cached = match refresh_strategy {
            RefreshStrategy::Online => None,
            RefreshStrategy::Offline | RefreshStrategy::OnlineIfUncached => {
                self.cache_manager.load_fresh_from(&source).await
            }
        };
        if let Some(cache) = cached {
            *self.remote_models.write().await = cache.models.clone();
            *self.remote_source.write().await = Some(source);
            *self.etag.write().await = cache.etag.clone();
            *self.list_error.write().await = None;
            return Ok(());
        }
        if refresh_strategy == RefreshStrategy::Offline {
            // Nothing this server provided, and no asking allowed: make the
            // absence explicit rather than keeping models from elsewhere.
            self.clear_server_models().await;
            return Ok(());
        }
        // Clear first: a failed request must not leave models the user's server
        // never served (another provider's list, or a stale cache) as the
        // picker's answer.
        self.clear_server_models().await;
        match self.fetch_server_listed_models(config).await {
            Ok((models, etag)) => {
                *self.remote_models.write().await = models.clone();
                *self.remote_source.write().await = Some(source.clone());
                *self.etag.write().await = etag.clone();
                *self.list_error.write().await = None;
                self.cache_manager
                    .persist_cache(&models, etag, Some(source))
                    .await;
                Ok(())
            }
            Err(err) => {
                let base_url = self.effective_provider(config).base_url.unwrap_or_default();
                *self.list_error.write().await = Some(format!(
                    "could not get the model list from {base_url}/models: {err}"
                ));
                Err(err)
            }
        }
    }

    async fn fetch_server_listed_models(
        &self,
        config: &Config,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        let provider = self.effective_provider(config);
        let api_provider = provider.to_api_provider()?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let mut request = api_provider.build_request(Method::GET, "models");
        if let Some(token) = provider
            .api_key()?
            .or_else(|| provider.experimental_bearer_token.clone())
        {
            let header_value =
                HeaderValue::from_str(&format!("Bearer {token}")).map_err(|err| {
                    CodexErr::InvalidRequest(format!(
                        "invalid API key configured for {}: {err}",
                        provider.name
                    ))
                })?;
            request.headers.insert(AUTHORIZATION, header_value);
        }

        let response = timeout(MODELS_REFRESH_TIMEOUT, transport.execute(request))
            .await
            .map_err(|_| CodexErr::Timeout)?
            .map_err(|err| map_api_error(err.into()))?;

        let models = parse_server_model_listing(&response.body);
        if models.is_empty() {
            let preview: String = String::from_utf8_lossy(&response.body)
                .chars()
                .take(200)
                .collect();
            return Err(CodexErr::InvalidRequest(format!(
                "{} `/models` returned no usable models: {preview}",
                provider.name
            )));
        }
        // A server that labels its model list with an ETag lets owl skip a refetch
        // while the list is unchanged; see `refresh_if_new_etag`.
        let etag = response
            .headers
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Ok((models, etag))
    }

    async fn get_etag(&self) -> Option<String> {
        self.etag.read().await.clone()
    }

    /// Turn the current model list into picker-ready presets.
    ///
    /// owl ships no catalog of its own, so these presets are exactly the models
    /// the selected provider listed; the first one visible in the picker becomes
    /// the default.
    fn build_available_models(&self, mut remote_models: Vec<ModelInfo>) -> Vec<ModelPreset> {
        remote_models.sort_by(|a, b| a.priority.cmp(&b.priority));

        let mut merged_presets: Vec<ModelPreset> =
            remote_models.into_iter().map(Into::into).collect();

        for preset in &mut merged_presets {
            preset.is_default = false;
        }
        if let Some(default) = merged_presets
            .iter_mut()
            .find(|preset| preset.show_in_picker)
        {
            default.is_default = true;
        } else if let Some(default) = merged_presets.first_mut() {
            default.is_default = true;
        }

        merged_presets
    }

    async fn get_remote_models(&self, config: &Config) -> Vec<ModelInfo> {
        // Only a list that came from this server may be used; a mismatch
        // (another provider's list, a stale cache) yields no models.
        if self.server_model_list_selected(config) && self.remote_models_match_server(config).await
        {
            self.remote_models.read().await.clone()
        } else {
            Vec::new()
        }
    }

    fn try_get_remote_models(&self, config: &Config) -> Result<Vec<ModelInfo>, TryLockError> {
        let expected = self.models_cache_source(config);
        let matches = self
            .remote_source
            .try_read()?
            .as_ref()
            .is_some_and(|source| *source == expected);
        Ok(if self.server_model_list_selected(config) && matches {
            self.remote_models.try_read()?.clone()
        } else {
            Vec::new()
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Construct a manager with a specific provider for testing.
    pub fn with_provider(codex_home: PathBuf, provider: ModelProviderInfo) -> Self {
        let cache_path = codex_home.join(MODEL_CACHE_FILE);
        let cache_manager = ModelsCacheManager::new(cache_path, DEFAULT_MODEL_CACHE_TTL);
        Self {
            remote_models: RwLock::new(Vec::new()),
            remote_source: RwLock::new(None),
            list_error: RwLock::new(None),
            etag: RwLock::new(None),
            cache_manager,
            provider: Some(provider),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Stand in for the user's server: put a model list into the manager as if this
    /// provider had just served it.
    ///
    /// owl's only source of per-model metadata is the `{base_url}/models` listing, so
    /// tests that care about what a model supports (reasoning levels, verbosity,
    /// context window) declare it here the way a gateway would, instead of relying on a
    /// bundled catalog owl no longer has.
    pub fn seed_server_models_for_testing(&self, config: &Config, models: Vec<ModelInfo>) {
        /// Store `value` in `lock`. Seeding happens before a turn starts, so no
        /// other writer can be holding these locks; a failure is a bug in the test.
        fn store<T>(lock: &RwLock<T>, value: T) {
            let mut guard = match lock.try_write() {
                Ok(guard) => guard,
                Err(err) => panic!("model list was in use while a test seeded it: {err}"),
            };
            *guard = value;
        }

        let source = self.models_cache_source(config);
        store(&self.remote_models, models);
        store(&self.remote_source, Some(source));
        store(&self.list_error, None);
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Get model identifier without consulting remote state or cache.
    ///
    /// owl ships no model list of its own: with no configured model there is
    /// nothing to fall back to, which callers report as an empty model list.
    pub fn get_model_offline(model: Option<&str>) -> String {
        model.unwrap_or_default().to_string()
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Build `ModelInfo` without consulting remote state or cache.
    pub fn construct_model_info_offline(model: &str, config: &Config) -> ModelInfo {
        model_info::with_config_overrides(model_info::find_model_info_for_slug(model), config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;
    use crate::config::ConfigOverrides;
    use crate::features::Feature;
    use crate::model_provider_info::WireApi;
    use chrono::Utc;
    use codex_protocol::openai_models::ModelsResponse;
    use core_test_support::responses::mount_models_once;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn remote_model(slug: &str, display: &str, priority: i32) -> ModelInfo {
        remote_model_with_visibility(slug, display, priority, "list")
    }

    fn remote_model_with_visibility(
        slug: &str,
        display: &str,
        priority: i32,
        visibility: &str,
    ) -> ModelInfo {
        serde_json::from_value(json!({
            "slug": slug,
            "display_name": display,
            "description": format!("{display} desc"),
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}],
            "shell_type": "shell_command",
            "visibility": visibility,
            "minimal_client_version": [0, 1, 0],
            "supported_in_api": true,
            "priority": priority,
            "base_instructions": "base instructions",
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10_000},
            "supports_parallel_tool_calls": false,
            "context_window": 272_000,
            "experimental_supported_tools": [],
        }))
        .expect("valid model")
    }

    fn assert_models_contain(actual: &[ModelInfo], expected: &[ModelInfo]) {
        for model in expected {
            assert!(
                actual.iter().any(|candidate| candidate.slug == model.slug),
                "expected model {} in cached list",
                model.slug
            );
        }
    }

    fn provider_for(base_url: String) -> ModelProviderInfo {
        ModelProviderInfo {
            name: "mock".into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(0),
            stream_max_retries: Some(0),
            stream_idle_timeout_ms: Some(5_000),
        }
    }

    #[tokio::test]
    async fn gate_uses_config_provider_when_no_override() {
        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        let manager = ModelsManager::new(codex_home.path().to_path_buf());

        // Ordinary runs inject no provider, so the gate (and the fetch) must use
        // the provider resolved from Config, not a hardcoded OpenAI one.
        assert!(manager.openwebui_provider_selected(&config));
        assert_eq!(
            manager.effective_provider(&config).wire_api,
            WireApi::Chat,
            "config-resolved Open WebUI provider should be used in ordinary runs"
        );
    }

    #[tokio::test]
    async fn injected_test_provider_takes_precedence_over_config() {
        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        let manager = ModelsManager::with_provider(
            codex_home.path().to_path_buf(),
            provider_for("http://mock.test/v1".to_string()),
        );

        // Integration tests inject an OpenAI-style (Responses wire) provider; the
        // gate must follow the injected provider so those tests never fetch the
        // default localhost endpoint.
        assert!(!manager.openwebui_provider_selected(&config));
        assert_eq!(
            manager.effective_provider(&config).base_url,
            Some("http://mock.test/v1".to_string()),
            "injected provider should win for tests"
        );
    }

    #[tokio::test]
    async fn openwebui_default_model_is_taken_from_the_users_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "id": "mock-gpt-mini", "object": "model" },
                    { "id": "mock-second", "object": "model" },
                ]
            })))
            .mount(&server)
            .await;

        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        assert_eq!(config.model, None, "a fresh install sets no model");
        let manager = ModelsManager::with_provider(
            codex_home.path().to_path_buf(),
            ModelProviderInfo {
                wire_api: WireApi::Chat,
                ..provider_for(format!("{}/", server.uri()))
            },
        );

        // A bundled ChatGPT model name does not exist on the user's server, so a
        // session that starts against Open WebUI must take its first model from
        // the server's own list.
        let default_model = manager
            .get_default_model(&config.model, &config, RefreshStrategy::OnlineIfUncached)
            .await;

        assert_eq!(default_model, "mock-gpt-mini");
    }

    #[tokio::test]
    async fn openwebui_uses_its_cache_before_asking_the_server_again() {
        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        // Nothing is listening here, so if the manager tried to refetch the default
        // model would come from the bundled catalog and the assert below would fail.
        let manager = ModelsManager::with_provider(
            codex_home.path().to_path_buf(),
            ModelProviderInfo {
                wire_api: WireApi::Chat,
                ..provider_for("http://127.0.0.1:1/v1".to_string())
            },
        );
        let cached =
            parse_server_model_listing(br#"{"data":[{"id":"qwen3:8b"},{"id":"llama3.2"}]}"#);
        manager
            .cache_manager
            .persist_cache(&cached, None, Some(manager.models_cache_source(&config)))
            .await;

        let default_model = manager
            .get_default_model(&config.model, &config, RefreshStrategy::OnlineIfUncached)
            .await;

        assert_eq!(default_model, "qwen3:8b");
    }

    /// A model list cached by *someone else* must never become owl's model list.
    ///
    /// The settings home can already hold a `models_cache.json` written by an
    /// upstream owl install (the ChatGPT catalog) or by a different server. Reusing
    /// it is what made owl answer `/model` with gpt model names right after the user
    /// had entered their own Open WebUI URL and token.
    #[tokio::test]
    async fn openwebui_refetches_when_the_cached_list_came_from_elsewhere() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "id": "qwen3:8b", "object": "model" }]
            })))
            .mount(&server)
            .await;

        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        let manager = ModelsManager::with_provider(
            codex_home.path().to_path_buf(),
            ModelProviderInfo {
                wire_api: WireApi::Chat,
                ..provider_for(format!("{}/", server.uri()))
            },
        );

        // Written the way a different provider would write it: no source of its own.
        let foreign = parse_server_model_listing(br#"{"data":[{"id":"gpt-5.2-codex"}]}"#);
        manager
            .cache_manager
            .persist_cache(&foreign, None, None)
            .await;

        let default_model = manager
            .get_default_model(&config.model, &config, RefreshStrategy::OnlineIfUncached)
            .await;

        assert_eq!(default_model, "qwen3:8b");
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/models")
                .count(),
            1,
            "the server's own model list is the only list owl may use"
        );
    }

    /// When the user's server cannot be reached, owl says so and offers no models,
    /// rather than starting a session on a model name that server has never heard of.
    #[tokio::test]
    async fn openwebui_reports_a_model_list_it_could_not_fetch() {
        let codex_home = tempdir().expect("temp dir");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                model_provider: Some(OPENWEBUI_PROVIDER_ID.to_string()),
                ..Default::default()
            })
            .build()
            .await
            .expect("load default test config");
        let manager = ModelsManager::with_provider(
            codex_home.path().to_path_buf(),
            ModelProviderInfo {
                wire_api: WireApi::Chat,
                ..provider_for("http://127.0.0.1:1/v1".to_string())
            },
        );

        let models = manager
            .list_models(&config, RefreshStrategy::OnlineIfUncached)
            .await;
        let slugs: Vec<&str> = models.iter().map(|model| model.model.as_str()).collect();
        assert!(models.is_empty(), "expected no models, got {slugs:?}");

        let error = manager
            .model_list_error()
            .await
            .expect("a reason for the missing model list");
        assert!(
            error.contains("127.0.0.1:1"),
            "the message should name the server that did not answer: {error}"
        );

        let default_model = manager
            .get_default_model(&config.model, &config, RefreshStrategy::OnlineIfUncached)
            .await;
        assert_eq!(default_model, "");
    }

    /// A provider the user wrote into config.toml is answered by its own server too.
    ///
    /// Upstream sent these providers the Responses-catalog request, which a
    /// chat-completions server answered in a shape the decoder rejected
    /// (`missing field models`), leaving the bundled catalog as the picker's list.
    #[tokio::test]
    async fn user_defined_chat_provider_lists_models_from_its_own_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "id": "my-model", "object": "model" },
                    { "id": "my-other-model", "object": "model" },
                ]
            })))
            .mount(&server)
            .await;

        let codex_home = tempdir().expect("temp dir");
        std::fs::write(
            codex_home.path().join("config.toml"),
            format!(
                "model_provider = \"mywebui\"\n\n[model_providers.mywebui]\nname = \"My WebUI\"\nbase_url = \"{}/\"\nwire_api = \"chat\"\n",
                server.uri()
            ),
        )
        .expect("write config.toml");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("load config.toml");
        assert_eq!(config.model_provider_id, "mywebui");

        let manager = ModelsManager::new(codex_home.path().to_path_buf());

        let models = manager
            .list_models(&config, RefreshStrategy::OnlineIfUncached)
            .await;
        let slugs: Vec<&str> = models.iter().map(|model| model.model.as_str()).collect();
        assert_eq!(slugs, vec!["my-model", "my-other-model"]);
        assert!(
            !slugs.contains(&"gpt-5.2-codex"),
            "the bundled catalog is not this provider's list: {slugs:?}"
        );
    }

    #[tokio::test]
    async fn refresh_available_models_sorts_by_priority() {
        let server = MockServer::start().await;
        let remote_models = vec![
            remote_model("priority-low", "Low", 1),
            remote_model("priority-high", "High", 0),
        ];
        let models_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: remote_models.clone(),
            },
        )
        .await;

        let codex_home = tempdir().expect("temp dir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("load default test config");
        config.features.enable(Feature::RemoteModels);
        let provider = provider_for(server.uri());
        let manager = ModelsManager::with_provider(codex_home.path().to_path_buf(), provider);

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("refresh succeeds");
        let cached_remote = manager.get_remote_models(&config).await;
        assert_models_contain(&cached_remote, &remote_models);

        let available = manager
            .list_models(&config, RefreshStrategy::OnlineIfUncached)
            .await;
        let high_idx = available
            .iter()
            .position(|model| model.model == "priority-high")
            .expect("priority-high should be listed");
        let low_idx = available
            .iter()
            .position(|model| model.model == "priority-low")
            .expect("priority-low should be listed");
        assert!(
            high_idx < low_idx,
            "higher priority should be listed before lower priority"
        );
        assert_eq!(
            models_mock.requests().len(),
            1,
            "expected a single /models request"
        );
    }

    #[tokio::test]
    async fn refresh_available_models_uses_cache_when_fresh() {
        let server = MockServer::start().await;
        let remote_models = vec![remote_model("cached", "Cached", 5)];
        let models_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: remote_models.clone(),
            },
        )
        .await;

        let codex_home = tempdir().expect("temp dir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("load default test config");
        config.features.enable(Feature::RemoteModels);
        let provider = provider_for(server.uri());
        let manager = ModelsManager::with_provider(codex_home.path().to_path_buf(), provider);

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("first refresh succeeds");
        assert_models_contain(&manager.get_remote_models(&config).await, &remote_models);

        // Second call should read from cache and avoid the network.
        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("cached refresh succeeds");
        assert_models_contain(&manager.get_remote_models(&config).await, &remote_models);
        assert_eq!(
            models_mock.requests().len(),
            1,
            "cache hit should avoid a second /models request"
        );
    }

    #[tokio::test]
    async fn refresh_available_models_refetches_when_cache_stale() {
        let server = MockServer::start().await;
        let initial_models = vec![remote_model("stale", "Stale", 1)];
        let initial_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: initial_models.clone(),
            },
        )
        .await;

        let codex_home = tempdir().expect("temp dir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("load default test config");
        config.features.enable(Feature::RemoteModels);
        let provider = provider_for(server.uri());
        let manager = ModelsManager::with_provider(codex_home.path().to_path_buf(), provider);

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("initial refresh succeeds");

        // Rewrite cache with an old timestamp so it is treated as stale.
        manager
            .cache_manager
            .manipulate_cache_for_test(|fetched_at| {
                *fetched_at = Utc::now() - chrono::Duration::hours(1);
            })
            .await
            .expect("cache manipulation succeeds");

        let updated_models = vec![remote_model("fresh", "Fresh", 9)];
        server.reset().await;
        let refreshed_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: updated_models.clone(),
            },
        )
        .await;

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("second refresh succeeds");
        assert_models_contain(&manager.get_remote_models(&config).await, &updated_models);
        assert_eq!(
            initial_mock.requests().len(),
            1,
            "initial refresh should only hit /models once"
        );
        assert_eq!(
            refreshed_mock.requests().len(),
            1,
            "stale cache refresh should fetch /models once"
        );
    }

    #[tokio::test]
    async fn refresh_available_models_drops_removed_remote_models() {
        let server = MockServer::start().await;
        let initial_models = vec![remote_model("remote-old", "Remote Old", 1)];
        let initial_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: initial_models,
            },
        )
        .await;

        let codex_home = tempdir().expect("temp dir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("load default test config");
        config.features.enable(Feature::RemoteModels);
        let provider = provider_for(server.uri());
        let mut manager = ModelsManager::with_provider(codex_home.path().to_path_buf(), provider);
        manager.cache_manager.set_ttl(Duration::ZERO);

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("initial refresh succeeds");

        server.reset().await;
        let refreshed_models = vec![remote_model("remote-new", "Remote New", 1)];
        let refreshed_mock = mount_models_once(
            &server,
            ModelsResponse {
                models: refreshed_models,
            },
        )
        .await;

        manager
            .refresh_available_models(&config, RefreshStrategy::OnlineIfUncached)
            .await
            .expect("second refresh succeeds");

        let available = manager
            .try_list_models(&config)
            .expect("models should be available");
        assert!(
            available.iter().any(|preset| preset.model == "remote-new"),
            "new remote model should be listed"
        );
        assert!(
            !available.iter().any(|preset| preset.model == "remote-old"),
            "removed remote model should not be listed"
        );
        assert_eq!(
            initial_mock.requests().len(),
            1,
            "initial refresh should only hit /models once"
        );
        assert_eq!(
            refreshed_mock.requests().len(),
            1,
            "second refresh should only hit /models once"
        );
    }

    #[test]
    fn build_available_models_picks_default_after_hiding_hidden_models() {
        let codex_home = tempdir().expect("temp dir");
        let provider = provider_for("http://example.test".to_string());
        let manager = ModelsManager::with_provider(codex_home.path().to_path_buf(), provider);

        let hidden_model = remote_model_with_visibility("hidden", "Hidden", 0, "hide");
        let visible_model = remote_model_with_visibility("visible", "Visible", 1, "list");

        let expected_hidden = ModelPreset::from(hidden_model.clone());
        let mut expected_visible = ModelPreset::from(visible_model.clone());
        expected_visible.is_default = true;

        let available = manager.build_available_models(vec![hidden_model, visible_model]);

        assert_eq!(available, vec![expected_hidden, expected_visible]);
    }

    #[test]
    fn parse_server_model_listing_accepts_both_listing_shapes() {
        let wrapped =
            br#"{"data":[{"id":"qwen3:32b","name":"Qwen3 32B"},{"id":"llama3.2:latest"}]}"#;
        let models = parse_server_model_listing(wrapped);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].slug, "qwen3:32b");
        assert_eq!(models[0].display_name, "Qwen3 32B");
        assert_eq!(models[1].display_name, "llama3.2:latest");
        assert!(models.iter().all(|model| model.supported_in_api));

        let bare = br#"[{"id":"mistral"}]"#;
        let models = parse_server_model_listing(bare);
        let slugs: Vec<String> = models.iter().map(|model| model.slug.clone()).collect();
        assert_eq!(slugs, vec!["mistral".to_string()]);

        // The richer `{"models":[...]}` listing keeps the metadata the gateway describes.
        let gateway = serde_json::to_vec(&ModelsResponse {
            models: vec![
                remote_model("priority-low", "Low", 1),
                remote_model_with_visibility("hidden", "Hidden", 0, "hide"),
            ],
        })
        .expect("encode gateway listing");
        let models = parse_server_model_listing(&gateway);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].priority, 1, "the gateway's own priority is kept");
        assert_eq!(models[1].slug, "hidden");
        assert_eq!(models[1].visibility, ModelVisibility::Hide);
        assert_eq!(
            models[0].truncation_policy,
            remote_model("priority-low", "Low", 1).truncation_policy
        );

        // A partial entry of that shape is not model metadata owl can trust.
        assert!(
            parse_server_model_listing(br#"{"models":[{"slug":"halfway","priority":3}]}"#)
                .is_empty()
        );

        assert!(parse_server_model_listing(b"not json").is_empty());
        assert!(parse_server_model_listing(br#"{"data":[]}"#).is_empty());
    }
}
