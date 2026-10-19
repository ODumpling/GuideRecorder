//! API key storage in the macOS Keychain (Windows Credential Manager /
//! Secret Service elsewhere, via `keyring`): one entry per provider that
//! takes a key ([`ACCOUNT`] for Anthropic, [`OPENAI_COMPATIBLE_ACCOUNT`]),
//! and the web app sign-in for sync ([`SYNC_ACCOUNT`]).
//!
//! The key is wrapped in [`ApiKey`], whose `Debug` and `Display` never show
//! it, so it cannot end up in logs by accident.

use super::AiError;

pub const SERVICE: &str = "dev.dufflebear.guiderecorder";
/// The Anthropic key's entry.
pub const ACCOUNT: &str = "anthropic-api-key";
/// The key for a custom OpenAI-compatible API.
pub const OPENAI_COMPATIBLE_ACCOUNT: &str = "openai-compatible-api-key";
/// The web app session token the desktop app syncs with.
pub const SYNC_ACCOUNT: &str = "sync-token";

/// An API key. Formatting it prints `ApiKey(<redacted>)`.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap a key, trimming surrounding whitespace. Empty keys are rejected.
    pub fn new(key: impl Into<String>) -> Result<Self, AiError> {
        let key = key.into();
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Err(AiError::MissingApiKey);
        }
        if !trimmed.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(AiError::InvalidApiKey);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The raw key, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

impl std::fmt::Display for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

fn entry(account: &str) -> Result<keyring::Entry, AiError> {
    keyring::Entry::new(SERVICE, account).map_err(keychain_error)
}

// keyring errors describe the store, not the secret, so they are safe to show.
fn keychain_error(err: keyring::Error) -> AiError {
    AiError::Keychain(err.to_string())
}

/// The key stored under `account`, or `None` when none is set.
pub fn get_key(account: &str) -> Result<Option<ApiKey>, AiError> {
    match entry(account)?.get_password() {
        Ok(key) => ApiKey::new(key).map(Some).or(Ok(None)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(keychain_error(err)),
    }
}

/// Store `key` under `account`, replacing any existing one.
pub fn set_key(account: &str, key: &ApiKey) -> Result<(), AiError> {
    entry(account)?
        .set_password(key.expose())
        .map_err(keychain_error)
}

/// Remove the key stored under `account`. Succeeds when none is set.
pub fn delete_key(account: &str) -> Result<(), AiError> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(keychain_error(err)),
    }
}

/// The stored Anthropic key, or `None` when none is set.
pub fn get_api_key() -> Result<Option<ApiKey>, AiError> {
    get_key(ACCOUNT)
}

/// Store the Anthropic key, replacing any existing one.
pub fn set_api_key(key: &ApiKey) -> Result<(), AiError> {
    set_key(ACCOUNT, key)
}

/// Remove the stored Anthropic key. Succeeds when none is set.
pub fn delete_api_key() -> Result<(), AiError> {
    delete_key(ACCOUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_never_formatted() {
        let key = ApiKey::new("  sk-ant-secret\n").unwrap();
        assert_eq!(key.expose(), "sk-ant-secret");
        assert_eq!(format!("{key:?}"), "ApiKey(<redacted>)");
        assert_eq!(key.to_string(), "<redacted>");
        assert!(!format!("{:?}", Some(&key)).contains("secret"));
        assert_eq!(ApiKey::new("   "), Err(AiError::MissingApiKey));
        assert_eq!(ApiKey::new("sk ant"), Err(AiError::InvalidApiKey));
        assert_eq!(ApiKey::new("sk-ant-é"), Err(AiError::InvalidApiKey));
    }

    #[test]
    #[ignore = "touches the real login Keychain and may show a permission prompt"]
    fn keychain_round_trip() {
        let previous = get_api_key().unwrap();
        let key = ApiKey::new("sk-ant-guiderecorder-test").unwrap();
        set_api_key(&key).unwrap();
        assert_eq!(get_api_key().unwrap(), Some(key));
        delete_api_key().unwrap();
        assert_eq!(get_api_key().unwrap(), None);
        delete_api_key().unwrap();
        if let Some(previous) = previous {
            set_api_key(&previous).unwrap();
        }
    }
}
