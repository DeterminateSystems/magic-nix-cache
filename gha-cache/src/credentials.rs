//! Access credentials.

use std::env;

use derivative::Derivative;
use serde::{Deserialize, Serialize};

/// Credentials to access the GitHub Actions Cache.
#[derive(Clone, Derivative, Deserialize, Serialize)]
#[derivative(Debug)]
pub struct Credentials {
    /// The base URL of the cache.
    ///
    /// This is the `ACTIONS_CACHE_URL` environment variable.
    #[serde(alias = "ACTIONS_CACHE_URL")]
    pub(crate) cache_url: String,

    /// The token.
    ///
    /// This is the `ACTIONS_RUNTIME_TOKEN` environment variable.
    #[derivative(Debug = "ignore")]
    #[serde(alias = "ACTIONS_RUNTIME_TOKEN")]
    pub(crate) runtime_token: String,
}

impl Credentials {
    /// Tries to load credentials from the environment.
    pub fn load_from_env() -> Option<Self> {
        let cache_url = env::var("ACTIONS_CACHE_URL").ok()?;
        let runtime_token = env::var("ACTIONS_RUNTIME_TOKEN").ok()?;

        Some(Self {
            cache_url,
            runtime_token,
        })
    }
}
