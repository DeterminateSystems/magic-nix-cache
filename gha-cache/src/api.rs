//! GitHub Actions Cache API client.
//!
//! We expose a high-level API that deals with "files."

use std::fmt;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::credentials::Credentials;
use crate::github::actions::results::api::v1::{
    CacheServiceClient, CreateCacheEntryRequest, FinalizeCacheEntryUploadRequest,
    GetCacheEntryDownloadUrlRequest,
};
use crate::util::read_chunk_async;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future;
use rand::{distributions::Alphanumeric, Rng};
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE},
    Client, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{io::AsyncRead, sync::Semaphore};
use twirp::client::Client as TwirpClient;
use unicode_bom::Bom;
use url::Url;

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
const CHUNK_SIZE: usize = 32 * 1024 * 1024;

/// The number of chunks to upload at the same time.
const MAX_CONCURRENCY: usize = 4;

type Result<T> = std::result::Result<T, Error>;

pub type CircuitBreakerTrippedCallback = Arc<Box<dyn Fn() + Send + Sync>>;

/// An API error.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to initialize the client: {0}")]
    InitError(Box<dyn std::error::Error + Send + Sync>),

    #[error(
        "GitHub Actions Cache throttled Magic Nix Cache. Not trying to use it again on this run."
    )]
    CircuitBreakerTripped,

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

    #[error("API error: 'not ok' response")]
    ApiErrorNotOk,

    #[error("Twirp error: {0}")]
    TwirpError(#[from] twirp::ClientError),

    #[error("I/O error: {0}, context: {1}")]
    IoError(std::io::Error, String),

    #[error("Too many collisions")]
    TooManyCollisions,
}

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

    /// The TWIRP client for v2 cache service.
    twirp_client: TwirpClient,

    /// The concurrent upload limit.
    concurrency_limit: Arc<Semaphore>,

    circuit_breaker_429_tripped: Arc<AtomicBool>,

    circuit_breaker_429_tripped_callback: CircuitBreakerTrippedCallback,

    /// Backend request statistics.
    #[cfg(debug_assertions)]
    stats: RequestStats,
}

/// A file allocation.
#[derive(Debug, Clone)]
pub enum FileAllocation {
    V1(CacheId),
    V2(SignedUrl),
}

/// The ID of a cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CacheId(pub i64);

// A signed URL for a cache file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedUrl {
    pub signed_url: String,
    pub key: String,
}

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
    put: AtomicUsize,
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
    pub fn new(
        credentials: Credentials,
        circuit_breaker_429_tripped_callback: CircuitBreakerTrippedCallback,
    ) -> Result<Self> {
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

        // Create HTTP client with authorization header
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", credentials.runtime_token)
                .parse()
                .map_err(Error::init_error)?,
        );

        let service_url = credentials.results_url.clone() + "twirp/";

        let twirp_client = TwirpClient::new(
            reqwest::Url::parse(&service_url).map_err(Error::init_error)?,
            client.clone(),
            vec![],
        )
        .map_err(Error::init_error)?;

        Ok(Self {
            credentials,
            version: initial_version,
            version_hasher,
            client,
            twirp_client,
            concurrency_limit: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
            circuit_breaker_429_tripped: Arc::new(AtomicBool::from(false)),
            circuit_breaker_429_tripped_callback,
            #[cfg(debug_assertions)]
            stats: Default::default(),
        })
    }

    pub fn circuit_breaker_tripped(&self) -> bool {
        self.circuit_breaker_429_tripped.load(Ordering::Relaxed)
    }

    /// Mutates the cache version/namespace.
    pub fn mutate_version(&mut self, data: &[u8]) {
        self.version_hasher.update(data);
        self.version = hex::encode(self.version_hasher.clone().finalize());
    }

    // Public

    /// Allocates a file.
    pub async fn allocate_file(&self, key: &str) -> Result<FileAllocation> {
        self.reserve_cache(key, None).await
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

    /// Uploads a file. Returns the size of the file.
    pub async fn upload_file<S>(&self, allocation: FileAllocation, mut stream: S) -> Result<usize>
    where
        S: AsyncRead + Unpin + Send,
    {
        let mut offset = 0;

        if self.circuit_breaker_tripped() {
            return Err(Error::CircuitBreakerTripped);
        }

        match allocation {
            FileAllocation::V1(cache_id) => {
                let mut futures = Vec::new();

                loop {
                    let buf = BytesMut::with_capacity(CHUNK_SIZE);
                    let chunk = read_chunk_async(&mut stream, buf).await.map_err(|e| {
                        Error::IoError(e, "Reading a chunk during upload".to_string())
                    })?;
                    if chunk.is_empty() {
                        offset += chunk.len();
                        break;
                    }

                    if offset == chunk.len() {
                        tracing::trace!("Received first chunk for cache {:?}", cache_id);
                    }

                    let chunk_len = chunk.len();

                    #[cfg(debug_assertions)]
                    self.stats.patch.fetch_add(1, Ordering::SeqCst);

                    futures.push({
                        let client = self.client.clone();
                        let concurrency_limit = self.concurrency_limit.clone();
                        let circuit_breaker_429_tripped = self.circuit_breaker_429_tripped.clone();
                        let circuit_breaker_429_tripped_callback =
                            self.circuit_breaker_429_tripped_callback.clone();
                        let url = self.construct_url(&format!("caches/{}", cache_id.0));

                        tokio::task::spawn(async move {
                            let permit = concurrency_limit
                                .acquire()
                                .await
                                .expect("failed to acquire concurrency semaphore permit");

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

                            circuit_breaker_429_tripped
                                .check_result(&r, &circuit_breaker_429_tripped_callback);

                            r
                        })
                    });

                    offset += chunk_len;
                }

                future::join_all(futures)
                    .await
                    .into_iter()
                    .try_for_each(|join_result| {
                        join_result.expect("failed collecting a join result during parallel upload")
                    })?;

                tracing::debug!("Received all chunks for cache {:?}", cache_id);

                let req = CommitCacheRequest { size: offset };

                #[cfg(debug_assertions)]
                self.stats.post.fetch_add(1, Ordering::SeqCst);

                if let Err(e) = self
                    .client
                    .post(self.construct_url(&format!("caches/{}", cache_id.0)))
                    .json(&req)
                    .send()
                    .await?
                    .check()
                    .await
                {
                    self.circuit_breaker_429_tripped
                        .check_err(&e, &self.circuit_breaker_429_tripped_callback);
                    return Err(e);
                }

                Ok(offset)
            }
            FileAllocation::V2(SignedUrl { signed_url, key }) => {
                let url = Url::parse(&signed_url).map_err(Error::init_error)?;

                let client = Client::builder()
                    .user_agent(USER_AGENT)
                    .build()
                    .map_err(Error::init_error)?;

                client
                    .put(url.clone())
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .header(CONTENT_LENGTH, 0)
                    .header("x-ms-blob-type", "AppendBlob")
                    .send()
                    .await?
                    .check()
                    .await?;

                let mut append_url = url.clone();
                append_url
                    .query_pairs_mut()
                    .append_pair("comp", "appendblock");

                loop {
                    let buf = BytesMut::with_capacity(CHUNK_SIZE);
                    let chunk = read_chunk_async(&mut stream, buf).await.map_err(|e| {
                        Error::IoError(e, "Reading a chunk during upload".to_string())
                    })?;
                    if chunk.is_empty() {
                        offset += chunk.len();
                        break;
                    }

                    if offset == chunk.len() {
                        tracing::trace!("Received first chunk for cache {:?}", key);
                    }

                    let chunk_len = chunk.len();

                    #[cfg(debug_assertions)]
                    self.stats.put.fetch_add(1, Ordering::SeqCst);

                    client
                        .put(append_url.clone())
                        .header(CONTENT_TYPE, "application/octet-stream")
                        .header(CONTENT_LENGTH, chunk_len as u64)
                        .header("x-ms-blob-type", "AppendBlob")
                        .body(chunk)
                        .send()
                        .await?
                        .check()
                        .await?;

                    offset += chunk_len;
                }

                let mut finalize_url = url.clone();
                finalize_url.query_pairs_mut().append_pair("comp", "seal");

                client
                    .put(finalize_url)
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .header(CONTENT_LENGTH, 0)
                    .header("x-ms-blob-type", "AppendBlob")
                    .send()
                    .await?
                    .check()
                    .await?;

                let request = FinalizeCacheEntryUploadRequest {
                    metadata: None,
                    key,
                    size_bytes: offset as i64,
                    version: self.version.clone(),
                };

                let response = self.twirp_client.finalize_cache_entry_upload(request).await;

                match response {
                    Ok(response) => {
                        if response.ok {
                            Ok(offset)
                        } else {
                            Err(Error::ApiErrorNotOk)
                        }
                    }
                    Err(e) => Err(e.into()),
                }
            }
        }
    }

    /// Downloads a file based on a list of key prefixes.
    pub async fn get_file_url(&self, keys: &[&str]) -> Result<Option<String>> {
        if self.circuit_breaker_tripped() {
            return Err(Error::CircuitBreakerTripped);
        }

        self.get_cache_entry(keys).await
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
    async fn get_cache_entry(&self, keys: &[&str]) -> Result<Option<String>> {
        if self.circuit_breaker_tripped() {
            return Err(Error::CircuitBreakerTripped);
        }

        #[cfg(debug_assertions)]
        self.stats.get.fetch_add(1, Ordering::SeqCst);

        if self.credentials.service_v2.is_empty() {
            let res = self
                .client
                .get(self.construct_url("cache"))
                .query(&[("version", &self.version), ("keys", &keys.join(","))])
                .send()
                .await?
                .check_json::<ArtifactCacheEntry>()
                .await;

            self.circuit_breaker_429_tripped
                .check_result(&res, &self.circuit_breaker_429_tripped_callback);

            match res {
                Ok(entry) => Ok(Some(entry.archive_location)),
                Err(Error::DecodeError { status, .. }) if status == StatusCode::NO_CONTENT => {
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        } else {
            let res = self
                .twirp_client
                .get_cache_entry_download_url(GetCacheEntryDownloadUrlRequest {
                    version: self.version.clone(),
                    key: keys[0].to_string(),
                    restore_keys: keys.iter().map(|k| k.to_string()).collect(),
                    metadata: None,
                })
                .await;

            match res {
                Ok(entry) => {
                    if entry.ok {
                        Ok(Some(entry.signed_download_url))
                    } else {
                        Ok(None)
                    }
                }
                Err(e) => Err(e.into()),
            }
        }
    }

    /// Reserves a new cache.
    ///
    /// The cache key should be unique. A cache cannot be created
    /// again if the same (cache_name, cache_version) pair already
    /// exists.
    async fn reserve_cache(&self, key: &str, cache_size: Option<usize>) -> Result<FileAllocation> {
        if self.circuit_breaker_tripped() {
            return Err(Error::CircuitBreakerTripped);
        }

        if self.credentials.service_v2.is_empty() {
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
                .check_json::<ReserveCacheResponse>()
                .await;

            self.circuit_breaker_429_tripped
                .check_result(&res, &self.circuit_breaker_429_tripped_callback);

            Ok(FileAllocation::V1(res?.cache_id))
        } else {
            let req = CreateCacheEntryRequest {
                metadata: None,
                key: key.to_string(),
                version: self.version.clone(),
            };

            let res = self.twirp_client.create_cache_entry(req).await?;

            Ok(FileAllocation::V2(SignedUrl {
                signed_url: res.signed_upload_url,
                key: key.to_string(),
            }))
        }
    }

    fn construct_url(&self, resource: &str) -> String {
        let mut url = self.credentials.cache_url.clone();
        if !url.ends_with('/') {
            url.push('/');
        }
        url.push_str("_apis/artifactcache/");
        url.push_str(resource);
        url
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

trait AtomicCircuitBreaker {
    fn check_err(&self, e: &Error, callback: &CircuitBreakerTrippedCallback);
    fn check_result<T>(
        &self,
        r: &std::result::Result<T, Error>,
        callback: &CircuitBreakerTrippedCallback,
    );
}

impl AtomicCircuitBreaker for AtomicBool {
    fn check_result<T>(
        &self,
        r: &std::result::Result<T, Error>,
        callback: &CircuitBreakerTrippedCallback,
    ) {
        if let Err(ref e) = r {
            self.check_err(e, callback)
        }
    }

    fn check_err(&self, e: &Error, callback: &CircuitBreakerTrippedCallback) {
        if let Error::ApiError {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            ..
        } = e
        {
            tracing::info!("Disabling GitHub Actions Cache due to 429: Too Many Requests");
            self.store(true, Ordering::Relaxed);
            callback();
        }
    }
}
