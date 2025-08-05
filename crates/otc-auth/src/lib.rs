use otc_models::ApiKey;
use snafu::{prelude::*, Whatever};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub enum AuthError {
    #[snafu(display("Market maker '{}' not found", market_maker))]
    MarketMakerNotFound { market_maker: String },

    #[snafu(display("Invalid API key for market maker '{}'", market_maker))]
    InvalidApiKey { market_maker: String },

    #[snafu(display("API key ID '{}' not found", id))]
    ApiKeyIdNotFound { id: Uuid },

    #[snafu(display("Invalid API key for ID '{}'", id))]
    InvalidApiKeyForId { id: Uuid },
}

type Result<T, E = AuthError> = std::result::Result<T, E>;

/// API key store that loads keys from a JSON file
pub struct ApiKeyStore {
    keys: HashMap<String, ApiKey>,
    keys_by_id: HashMap<Uuid, ApiKey>,
}

impl ApiKeyStore {
    /// Create a new API key store from a JSON file
    pub async fn new(whitelist_file_path: PathBuf) -> Result<Self, Whatever> {
        let api_keys_file =
            std::fs::read_to_string(&whitelist_file_path).whatever_context(format!(
                "Failed to read whitelist file {}",
                whitelist_file_path.display()
            ))?;
        let api_keys: Vec<ApiKey> = serde_json::from_str(&api_keys_file).whatever_context(
            format!("Invalid whitelist file {}", whitelist_file_path.display()),
        )?;

        let mut keys = HashMap::new();
        let mut keys_by_id = HashMap::new();

        for key in api_keys {
            keys.insert(key.market_maker.clone(), key.clone());
            keys_by_id.insert(key.id, key);
        }

        Ok(Self { keys, keys_by_id })
    }

    /// Validate an API key for a market maker
    pub fn validate(&self, market_maker: &str, api_key: &str) -> Result<()> {
        let stored_key = self
            .keys
            .get(market_maker)
            .context(MarketMakerNotFoundSnafu { market_maker })?;

        if stored_key.verify(api_key) {
            Ok(())
        } else {
            Err(AuthError::InvalidApiKey {
                market_maker: market_maker.to_string(),
            })
        }
    }

    /// Check if a market maker exists
    #[must_use]
    pub fn contains_market_maker(&self, market_maker: &str) -> bool {
        self.keys.contains_key(market_maker)
    }

    /// Validate an API key by UUID and return the market maker name
    pub fn validate_by_id(&self, id: &Uuid, api_key: &str) -> Result<String> {
        let stored_key = self
            .keys_by_id
            .get(id)
            .context(ApiKeyIdNotFoundSnafu { id: *id })?;

        if stored_key.verify(api_key) {
            Ok(stored_key.market_maker.clone())
        } else {
            Err(AuthError::InvalidApiKeyForId { id: *id })
        }
    }

    /// Get API key by UUID
    #[must_use]
    pub fn get_by_id(&self, id: &Uuid) -> Option<&ApiKey> {
        self.keys_by_id.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otc_models::ApiKey;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_api_key_store() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("whitelist.json");

        let api_keys = vec![ApiKey {
            id: Uuid::new_v4(),
            market_maker: "test_mm".to_string(),
            hash: "$argon2id$v=19$m=19456,t=2,p=1$test_salt$test_hash".to_string(),
        }];

        fs::write(&file_path, serde_json::to_string(&api_keys).unwrap()).unwrap();

        let store = ApiKeyStore::new(file_path).await.unwrap();
        assert!(store.contains_market_maker("test_mm"));
        assert!(!store.contains_market_maker("unknown_mm"));
    }
}
