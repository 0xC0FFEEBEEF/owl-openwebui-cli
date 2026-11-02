//! Owl authenticates with exactly one kind of credential: an API key.
//!
//! The key can be saved in the settings home (`auth.json`, see `storage.rs`),
//! or it can come from the model provider itself (`OPENWEBUI_API_KEY`, a custom
//! provider's `api_key`/`env_key`). There is no account, no login flow, and no
//! token refresh: if the key is wrong, the provider says so and the turn fails.
mod storage;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use codex_app_server_protocol::AuthMode;

pub use crate::auth::storage::AuthCredentialsStoreMode;
pub use crate::auth::storage::AuthDotJson;
use crate::auth::storage::create_auth_storage;

/// The API-key credential used to talk to a model provider.
#[derive(Debug, Clone)]
pub struct CodexAuth {
    pub(crate) api_key: Option<String>,
}

impl PartialEq for CodexAuth {
    fn eq(&self, other: &Self) -> bool {
        self.api_key == other.api_key
    }
}

impl CodexAuth {
    /// Loads the available auth information from auth storage.
    pub fn from_auth_storage(
        codex_home: &Path,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<Option<CodexAuth>> {
        load_auth(codex_home, auth_credentials_store_mode)
    }

    pub fn get_token(&self) -> Result<String, std::io::Error> {
        Ok(self.api_key.clone().unwrap_or_default())
    }

    pub fn from_api_key(api_key: &str) -> Self {
        Self {
            api_key: Some(api_key.to_owned()),
        }
    }
}

/// Delete the auth.json file inside `codex_home` if it exists. Returns `Ok(true)`
/// if a file was removed, `Ok(false)` if no auth file was present.
pub fn logout(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<bool> {
    let storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    storage.delete()
}

/// Writes an `auth.json` that contains only the API key.
pub fn login_with_api_key(
    codex_home: &Path,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<()> {
    let auth_dot_json = AuthDotJson {
        api_key: Some(api_key.to_string()),
    };
    save_auth(codex_home, &auth_dot_json, auth_credentials_store_mode)
}

/// Persist the provided auth payload using the specified backend.
pub fn save_auth(
    codex_home: &Path,
    auth: &AuthDotJson,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<()> {
    let storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    storage.save(auth)
}

/// Load CLI auth data using the configured credential store backend.
/// Returns `None` when no credentials are stored. This function is
/// provided only for tests. Production code should not directly load
/// from the auth.json storage. It should use the AuthManager abstraction
/// instead.
pub fn load_auth_dot_json(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<Option<AuthDotJson>> {
    let storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);
    storage.load()
}

fn load_auth(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<Option<CodexAuth>> {
    let storage = create_auth_storage(codex_home.to_path_buf(), auth_credentials_store_mode);

    let Some(auth_dot_json) = storage.load()? else {
        return Ok(None);
    };

    // Only a stored API key is honored. Auth files that carry no API key (for
    // example, left over from other tools) are ignored.
    Ok(auth_dot_json.api_key.map(|api_key| CodexAuth {
        api_key: Some(api_key),
    }))
}

/// Internal cached auth state.
#[derive(Clone, Debug)]
struct CachedAuth {
    auth: Option<CodexAuth>,
}

/// Central manager providing a single source of truth for the API key stored in
/// the settings home. It loads once and then hands out cloned `CodexAuth`
/// values so the rest of the program has a consistent snapshot.
///
/// External modifications to `auth.json` will NOT be observed until `reload()`
/// is called explicitly. This matches the design goal of avoiding different
/// parts of the program seeing inconsistent auth data mid-run.
#[derive(Debug)]
pub struct AuthManager {
    codex_home: PathBuf,
    inner: RwLock<CachedAuth>,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
}

impl AuthManager {
    /// Create a new manager loading the initial auth from storage. Errors
    /// loading auth are swallowed; `auth()` will simply return `None` in that
    /// case so callers can treat it as an unauthenticated state.
    pub fn new(codex_home: PathBuf, auth_credentials_store_mode: AuthCredentialsStoreMode) -> Self {
        let auth = load_auth(&codex_home, auth_credentials_store_mode)
            .ok()
            .flatten();
        Self {
            codex_home,
            inner: RwLock::new(CachedAuth { auth }),
            auth_credentials_store_mode,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Create an AuthManager with a specific CodexAuth, for testing only.
    pub fn from_auth_for_testing(auth: CodexAuth) -> Arc<Self> {
        let cached = CachedAuth { auth: Some(auth) };

        Arc::new(Self {
            codex_home: PathBuf::from("non-existent"),
            inner: RwLock::new(cached),
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Create an AuthManager with a specific CodexAuth and codex home, for testing only.
    pub fn from_auth_for_testing_with_home(auth: CodexAuth, codex_home: PathBuf) -> Arc<Self> {
        let cached = CachedAuth { auth: Some(auth) };
        Arc::new(Self {
            codex_home,
            inner: RwLock::new(cached),
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
        })
    }

    /// Current cached auth (clone). May be `None` if no key is stored or the
    /// store could not be read.
    pub fn auth_cached(&self) -> Option<CodexAuth> {
        self.inner.read().ok().and_then(|c| c.auth.clone())
    }

    /// Current cached auth (clone). May be `None` if no key is stored.
    pub async fn auth(&self) -> Option<CodexAuth> {
        self.auth_cached()
    }

    /// Force a reload of the auth information from auth.json. Returns
    /// whether the auth value changed.
    pub fn reload(&self) -> bool {
        tracing::info!("Reloading auth");
        let new_auth = self.load_auth_from_storage();
        self.set_auth(new_auth)
    }

    fn auths_equal(a: &Option<CodexAuth>, b: &Option<CodexAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    fn load_auth_from_storage(&self) -> Option<CodexAuth> {
        load_auth(&self.codex_home, self.auth_credentials_store_mode)
            .ok()
            .flatten()
    }

    fn set_auth(&self, new_auth: Option<CodexAuth>) -> bool {
        if let Ok(mut guard) = self.inner.write() {
            let changed = !AuthManager::auths_equal(&guard.auth, &new_auth);
            tracing::info!("Reloaded auth, changed: {changed}");
            guard.auth = new_auth;
            changed
        } else {
            false
        }
    }

    /// Convenience constructor returning an `Arc` wrapper.
    pub fn shared(
        codex_home: PathBuf,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> Arc<Self> {
        Arc::new(Self::new(codex_home, auth_credentials_store_mode))
    }

    /// Log out by deleting the on-disk auth.json (if present). Returns Ok(true)
    /// if a file was removed, Ok(false) if no auth file existed. On success,
    /// reloads the in-memory auth cache so callers immediately observe the
    /// unauthenticated state.
    pub fn logout(&self) -> std::io::Result<bool> {
        let removed = super::auth::logout(&self.codex_home, self.auth_credentials_store_mode)?;
        // Always reload to clear any cached auth (even if file absent).
        self.reload();
        Ok(removed)
    }

    pub fn get_auth_mode(&self) -> Option<AuthMode> {
        self.auth_cached().map(|_| AuthMode::ApiKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn login_with_api_key_overwrites_existing_auth_json() -> anyhow::Result<()> {
        let dir = TempDir::new()?;

        super::login_with_api_key(dir.path(), "sk-old", AuthCredentialsStoreMode::File)
            .expect("first login_with_api_key should succeed");
        super::login_with_api_key(dir.path(), "sk-new", AuthCredentialsStoreMode::File)
            .expect("second login_with_api_key should succeed");

        let auth = super::load_auth_dot_json(dir.path(), AuthCredentialsStoreMode::File)?
            .context("auth.json should exist")?;

        assert_eq!(auth.api_key.as_deref(), Some("sk-new"));
        Ok(())
    }

    #[test]
    fn missing_auth_json_returns_none() {
        let dir = TempDir::new().expect("TempDir should succeed");
        let auth =
            super::load_auth_dot_json(dir.path(), AuthCredentialsStoreMode::File).expect("load");
        assert_eq!(auth, None);
    }

    #[test]
    fn loads_api_key_from_auth_json() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let auth_file = dir.path().join("auth.json");
        std::fs::write(
            &auth_file,
            serde_json::to_string(&json!({"api_key": "sk-test-key"}))?,
        )?;

        let auth = CodexAuth::from_auth_storage(dir.path(), AuthCredentialsStoreMode::File)?
            .context("expected auth to be present")?;

        assert_eq!(auth.api_key, Some("sk-test-key".to_string()));
        assert_eq!(auth.get_token()?, "sk-test-key");
        Ok(())
    }

    #[test]
    fn auth_json_without_api_key_is_ignored() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let auth_file = dir.path().join("auth.json");
        std::fs::write(
            &auth_file,
            serde_json::to_string(&json!({"some_other_field": 1}))?,
        )?;

        let auth = CodexAuth::from_auth_storage(dir.path(), AuthCredentialsStoreMode::File)?;
        assert_eq!(auth, None);
        Ok(())
    }

    #[test]
    fn logout_removes_auth_file() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        super::login_with_api_key(dir.path(), "sk-test-key", AuthCredentialsStoreMode::File)
            .context("login_with_api_key should succeed")?;

        let auth_file = dir.path().join("auth.json");
        assert!(auth_file.exists());

        assert!(logout(dir.path(), AuthCredentialsStoreMode::File)?);
        assert!(!auth_file.exists());
        Ok(())
    }

    #[test]
    fn from_api_key_equality_compares_keys() {
        let a = CodexAuth::from_api_key("sk-a");
        let b = CodexAuth::from_api_key("sk-a");
        let c = CodexAuth::from_api_key("sk-c");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
