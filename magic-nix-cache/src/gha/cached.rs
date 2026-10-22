use std::{path::Path, time::Duration};

use anyhow::Context as _;
use attic::hash::Hash;
use gha_cache::Api;

use super::upstream::{fetch_metadata, matching_url};

pub(super) struct CachedPaths {
    client: reqwest::Client,
    enabled: bool,
}

impl CachedPaths {
    pub(super) fn new() -> reqwest::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
            enabled: true,
        })
    }

    pub(super) async fn contains(
        &mut self,
        api: &Api,
        key: &str,
        path: &Path,
        hash: &Hash,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        match tokio::time::timeout(Duration::from_secs(5), self.lookup(api, key, path, hash)).await
        {
            Ok(Ok(found)) => found,
            result => {
                self.enabled = false;
                tracing::warn!(path = %path.display(), error = ?result, "Cannot check GitHub cache; disabling upload filtering for the rest of this job");
                false
            }
        }
    }

    async fn lookup(&self, api: &Api, key: &str, path: &Path, hash: &Hash) -> anyhow::Result<bool> {
        let Some(url) = api.get_file_url(&[key]).await? else {
            return Ok(false);
        };
        let Some(body) = fetch_metadata(&self.client, &url).await? else {
            return Ok(false);
        };
        let Some(url) = matching_url(&body, path, hash)? else {
            return Ok(false);
        };
        let nar_key = url
            .strip_prefix("nar/")
            .filter(|key| !key.is_empty())
            .context("Invalid GitHub cache NAR key")?;
        // Metadata and archives are separate cache entries and can be evicted separately.
        Ok(api.get_file_url(&[nar_key]).await?.is_some())
    }
}
