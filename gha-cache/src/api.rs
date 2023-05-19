//! GitHub Actions Cache API client.
//!
//! We expose a high-level API that deals with "files."

use std::fmt;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future;
use rand::{distributions::Alphanumeric, Rng};
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_RANGE, CONTENT_TYPE},
    Client, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{io::AsyncRead, sync::Semaphore};
use unicode_bom::Bom;

use crate::credentials::Credentials;
use crate::util::read_chunk_async;

/// The API version we implement.
///
/// <https://github.com/actions/toolkit/blob/0d44da2b87f9ed48ae889d15c6cc19667aa37ec0/packages/cache/src/internal/cacheHttpClient.ts>
const API_VERSION: &str = "6.0-preview.1";

/// The User-Agent string for the client.
///
/// We want to be polite :)
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// The default cache version/namespace.
const DEFAULT_VERSION: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// The chunk size in bytes.
///
/// We greedily read this much from the input stream at a time.
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// The number of chunks to upload at the same time.
const MAX_CONCURRENCY: usize = 5;

type Result<T> = std::result::Result<T, Error>;

/// An API error.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to initialize the client: {0}")]
    InitError(Box<dyn std::error::Error + Send + Sync>),

    #[error("Request error: {0}")]
    RequestError(#[from] reqwest::Error), // TODO: Better errors

    #[error("Failed to decode response ({status}): {error}")]
    DecodeError {
        status: StatusCode,
        bytes: Bytes,
        error: serde_json::Error,
    },

    #[error("API error ({status}): {info}")]
    ApiError {
        status: StatusCode,
        info: ApiErrorInfo,
    },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Too many collisions")]
    TooManyCollisions,
}

#[derive(Debug)]
pub struct Api {
    /// Credentials to access the cache.
    credentials: Credentials,

    /// The version used for all caches.
    ///
    /// This value should be tied to everything that affects
    /// the compatibility of the cached objects.
    version: String,

    /// The hasher of the version.
    version_hasher: Sha256,

    /// The HTTP client for authenticated requests.
    client: Client,

    /// The concurrent upload limit.
    concurrency_limit: Arc<Semaphore>,

    /// Backend request statistics.
    #[cfg(debug_assertions)]
    stats: RequestStats,
}

/// A file allocation.
#[derive(Debug, Clone, Copy)]
pub struct FileAllocation(CacheId);

/// The ID of a cache.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(transparent)]
struct CacheId(pub i32);

/// An API error.
#[derive(Debug, Clone)]
pub enum ApiErrorInfo {
    /// An error that we couldn't decode.
    Unstructured(Bytes),

    /// A structured API error.
    Structured(StructuredApiError),
}

/// A structured API error.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StructuredApiError {
    /// A human-readable error message.
    message: String,
}

/// A cache entry.
///
/// A valid entry looks like:
///
/// ```text
/// ArtifactCacheEntry {
///     cache_key: Some("hello-224".to_string()),
///     scope: Some("refs/heads/main".to_string()),
///     cache_version: Some("gha-cache/0.1.0".to_string()),
///     creation_time: Some("2023-01-01T00:00:00.0000000Z".to_string()),
///     archive_location: Some(
///         "https://[...].blob.core.windows.net/[...]/[...]?sv=2019-07-07&sr=b&sig=[...]".to_string()
///     ),
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct ArtifactCacheEntry {
    /// The cache key.
    #[serde(rename = "cacheKey")]
    cache_key: Option<String>,

    /// The scope of the cache.
    ///
    /// It appears to be the branch name.
    scope: Option<String>,

    /// The version of the cache.
    #[serde(rename = "cacheVersion")]
    cache_version: Option<String>,

    /// The creation timestamp.
    #[serde(rename = "creationTime")]
    creation_time: Option<String>,

    /// The archive location.
    #[serde(rename = "archiveLocation")]
    archive_location: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReserveCacheRequest<'a> {
    /// The cache key.
    key: &'a str,

    /// The cache version.
    ///
    /// This value should be tied to everything that affects
    /// the compatibility of the cached objects.
    version: &'a str,

    /// The size of the cache, in bytes.
    #[serde(rename = "cacheSize")]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_size: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReserveCacheResponse {
    /// The reserved cache ID.
    #[serde(rename = "cacheId")]
    cache_id: CacheId,
}

#[derive(Debug, Clone, Serialize)]
struct CommitCacheRequest {
    size: usize,
}

#[cfg(debug_assertions)]
#[derive(Default, Debug)]
struct RequestStats {
    get: AtomicUsize,
    post: AtomicUsize,
    patch: AtomicUsize,
}

#[async_trait]
trait ResponseExt {
    async fn check(self) -> Result<()>;
    async fn check_json<T: DeserializeOwned>(self) -> Result<T>;
}

impl Error {
    fn init_error<E>(e: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::InitError(Box::new(e))
    }
}

impl fmt::Display for ApiErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unstructured(bytes) => {
                write!(f, "[Unstructured] {}", String::from_utf8_lossy(bytes))
            }
            Self::Structured(e) => {
                write!(f, "{:?}", e)
            }
        }
    }
}

impl Api {
    pub fn new(credentials: Credentials) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let auth_header = {
            let mut h = HeaderValue::from_str(&format!("Bearer {}", credentials.runtime_token))
                .map_err(Error::init_error)?;
            h.set_sensitive(true);
            h
        };
        headers.insert("Authorization", auth_header);
        headers.insert(
            "Accept",
            HeaderValue::from_str(&format!("application/json;api-version={}", API_VERSION))
                .map_err(Error::init_error)?,
        );

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .build()
            .map_err(Error::init_error)?;

        let version_hasher = Sha256::new_with_prefix(DEFAULT_VERSION.as_bytes());
        let initial_version = hex::encode(version_hasher.clone().finalize());

        Ok(Self {
            credentials,
            version: initial_version,
            version_hasher,
            client,
            concurrency_limit: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
            #[cfg(debug_assertions)]
            stats: Default::default(),
        })
    }

    /// Mutates the cache version/namespace.
    pub fn mutate_version(&mut self, data: &[u8]) {
        self.version_hasher.update(data);
        self.version = hex::encode(self.version_hasher.clone().finalize());
    }

    // Public

    /// Allocates a file.
    pub async fn allocate_file(&self, key: &str) -> Result<FileAllocation> {
        let reservation = self.reserve_cache(key, None).await?;
        Ok(FileAllocation(reservation.cache_id))
    }

    /// Allocates a file with a random suffix.
    ///
    /// This is a hack to allow for easy "overwriting" without
    /// deleting the original cache.
    pub async fn allocate_file_with_random_suffix(&self, key: &str) -> Result<FileAllocation> {
        for _ in 0..5 {
            let nonce: String = rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(4)
                .map(char::from)
                .collect();

            let full_key = format!("{}-{}", key, nonce);

            match self.allocate_file(&full_key).await {
                Ok(allocation) => {
                    return Ok(allocation);
                }
                Err(e) => {
                    if let Error::ApiError {
                        info: ApiErrorInfo::Structured(structured),
                        ..
                    } = &e
                    {
                        if structured.message.contains("Cache already exists") {
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }

        Err(Error::TooManyCollisions)
    }

    /// Uploads a file.
    pub async fn upload_file<S>(&self, allocation: FileAllocation, mut stream: S) -> Result<()>
    where
        S: AsyncRead + Unpin + Send,
    {
        let mut offset = 0;
        let mut futures = Vec::new();
        loop {
            let buf = BytesMut::with_capacity(CHUNK_SIZE);
            let chunk = read_chunk_async(&mut stream, buf).await?;

            if chunk.is_empty() {
                offset += chunk.len();
                break;
            }

            if offset == chunk.len() {
                tracing::trace!("Received first chunk for cache {:?}", allocation.0);
            }

            let chunk_len = chunk.len();

            #[cfg(debug_assertions)]
            self.stats.patch.fetch_add(1, Ordering::SeqCst);

            futures.push({
                let client = self.client.clone();
                let concurrency_limit = self.concurrency_limit.clone();
                let url = self.construct_url(&format!("caches/{}", allocation.0 .0));

                tokio::task::spawn(async move {
                    let permit = concurrency_limit.acquire().await.unwrap();

                    tracing::trace!(
                        "Starting uploading chunk {}-{}",
                        offset,
                        offset + chunk_len - 1
                    );

                    let r = client
                        .patch(url)
                        .header(CONTENT_TYPE, "application/octet-stream")
                        .header(
                            CONTENT_RANGE,
                            format!("bytes {}-{}/*", offset, offset + chunk.len() - 1),
                        )
                        .body(chunk)
                        .send()
                        .await?
                        .check()
                        .await;

                    tracing::trace!(
                        "Finished uploading chunk {}-{}: {:?}",
                        offset,
                        offset + chunk_len - 1,
                        r
                    );

                    drop(permit);

                    r
                })
            });

            offset += chunk_len;
        }

        future::join_all(futures)
            .await
            .into_iter()
            .map(|join_result| join_result.unwrap())
            .collect::<Result<()>>()?;

        tracing::debug!("Received all chunks for cache {:?}", allocation.0);

        self.commit_cache(allocation.0, offset).await?;

        Ok(())
    }

    /// Downloads a file based on a list of key prefixes.
    pub async fn get_file_url(&self, keys: &[&str]) -> Result<Option<String>> {
        Ok(self
            .get_cache_entry(keys)
            .await?
            .map(|entry| entry.archive_location))
    }

    /// Dumps statistics.
    ///
    /// This is for debugging only.
    pub fn dump_stats(&self) {
        #[cfg(debug_assertions)]
        tracing::trace!("Request stats: {:?}", self.stats);
    }

    // Private

    /// Retrieves a cache based on a list of key prefixes.
    async fn get_cache_entry(&self, keys: &[&str]) -> Result<Option<ArtifactCacheEntry>> {
        #[cfg(debug_assertions)]
        self.stats.get.fetch_add(1, Ordering::SeqCst);

        let res = self
            .client
            .get(self.construct_url("cache"))
            .query(&[("version", &self.version), ("keys", &keys.join(","))])
            .send()
            .await?
            .check_json()
            .await;

        match res {
            Ok(entry) => Ok(Some(entry)),
            Err(Error::DecodeError { status, .. }) if status == StatusCode::NO_CONTENT => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Reserves a new cache.
    ///
    /// The cache key should be unique. A cache cannot be created
    /// again if the same (cache_name, cache_version) pair already
    /// exists.
    async fn reserve_cache(
        &self,
        key: &str,
        cache_size: Option<usize>,
    ) -> Result<ReserveCacheResponse> {
        tracing::debug!("Reserving cache for {}", key);

        let req = ReserveCacheRequest {
            key,
            version: &self.version,
            cache_size,
        };

        #[cfg(debug_assertions)]
        self.stats.post.fetch_add(1, Ordering::SeqCst);

        let res = self
            .client
            .post(self.construct_url("caches"))
            .json(&req)
            .send()
            .await?
            .check_json()
            .await?;

        Ok(res)
    }

    /// Finalizes uploading to a cache.
    async fn commit_cache(&self, cache_id: CacheId, size: usize) -> Result<()> {
        tracing::debug!("Commiting cache {:?}", cache_id);

        let req = CommitCacheRequest { size };

        #[cfg(debug_assertions)]
        self.stats.post.fetch_add(1, Ordering::SeqCst);

        self.client
            .post(self.construct_url(&format!("caches/{}", cache_id.0)))
            .json(&req)
            .send()
            .await?
            .check()
            .await?;

        Ok(())
    }

    fn construct_url(&self, resource: &str) -> String {
        format!(
            "{}/_apis/artifactcache/{}",
            self.credentials.cache_url, resource
        )
    }
}

#[async_trait]
impl ResponseExt for reqwest::Response {
    async fn check(self) -> Result<()> {
        let status = self.status();

        if !status.is_success() {
            return Err(handle_error(self).await);
        }

        Ok(())
    }

    async fn check_json<T: DeserializeOwned>(self) -> Result<T> {
        let status = self.status();

        if !status.is_success() {
            return Err(handle_error(self).await);
        }

        // We don't do `Response::json()` directly to preserve
        // the original response payload for troubleshooting.
        let bytes = self.bytes().await?;
        match serde_json::from_slice(&bytes) {
            Ok(decoded) => Ok(decoded),
            Err(error) => Err(Error::DecodeError {
                status,
                error,
                bytes,
            }),
        }
    }
}

async fn handle_error(res: reqwest::Response) -> Error {
    let status = res.status();
    let bytes = match res.bytes().await {
        Ok(bytes) => {
            let bom = Bom::from(bytes.as_ref());
            bytes.slice(bom.len()..)
        }
        Err(e) => {
            return e.into();
        }
    };

    let info = match serde_json::from_slice(&bytes) {
        Ok(structured) => ApiErrorInfo::Structured(structured),
        Err(e) => {
            tracing::info!("failed to decode error: {}", e);
            ApiErrorInfo::Unstructured(bytes)
        }
    };

    Error::ApiError { status, info }
}
