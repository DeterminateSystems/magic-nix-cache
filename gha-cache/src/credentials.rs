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

    /// The base URL of the v2 cache service.
    ///
    /// This is the `ACTIONS_RESULTS_URL` environment variable.
    #[serde(alias = "ACTIONS_RESULTS_URL")]
    pub(crate) results_url: String,

    /// The token.
    ///
    /// This is the `ACTIONS_RUNTIME_TOKEN` environment variable.
    #[derivative(Debug = "ignore")]
    #[serde(alias = "ACTIONS_RUNTIME_TOKEN")]
    pub(crate) runtime_token: String,

    /// Whether to use v2 or not.
    ///
    /// This is the `ACTIONS_CACHE_SERVICE_V2` environment variable.
    #[serde(alias = "ACTIONS_CACHE_SERVICE_V2")]
    pub(crate) service_v2: String,
}

impl Credentials {
    /// Tries to load credentials from the environment.
    pub fn load_from_env() -> Option<Self> {
        let cache_url = env::var("ACTIONS_CACHE_URL").ok()?;
        let results_url = env::var("ACTIONS_RESULTS_URL").ok()?;
        let runtime_token = env::var("ACTIONS_RUNTIME_TOKEN").ok()?;
        let service_v2 = env::var("ACTIONS_CACHE_SERVICE_V2").ok()?;

        Some(Self {
            cache_url,
            results_url,
            runtime_token,
            service_v2,
        })
    }
}
