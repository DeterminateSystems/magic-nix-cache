use std::{path::Path, time::Duration};

use anyhow::{anyhow, Context as _};
use attic::hash::Hash;

pub(super) struct UpstreamCache {
    client: reqwest::Client,
    base_url: Option<String>,
}

impl UpstreamCache {
    pub(super) fn new(base_url: Option<String>) -> reqwest::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
            base_url,
        })
    }

    pub(super) async fn contains(&mut self, store_path: &Path, nar_hash: &Hash) -> bool {
        let Some(base_url) = &self.base_url else {
            return false;
        };
        match self.lookup(base_url, store_path, nar_hash).await {
            Ok(found) => found,
            Err(error) => {
                self.base_url = None;
                tracing::warn!(path = %store_path.display(), %error, "Cannot check upstream cache; disabling upload filtering for the rest of this job");
                false
            }
        }
    }

    async fn lookup(
        &self,
        base_url: &str,
        store_path: &Path,
        nar_hash: &Hash,
    ) -> anyhow::Result<bool> {
        let hash = store_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split_once('-'))
            .map(|(hash, _)| hash)
            .context("Invalid store path")?;
        let url = format!("{}/{hash}.narinfo", base_url.trim_end_matches('/'));
        let Some(body) = fetch_metadata(&self.client, &url).await? else {
            return Ok(false);
        };
        metadata_matches(&body, store_path, nar_hash)
    }
}

pub(super) async fn fetch_metadata(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<Option<String>> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.without_url())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response = response
        .error_for_status()
        .map_err(|error| error.without_url())?;
    // Bound metadata size even when the server omits Content-Length.
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())?
    {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err(anyhow!("Cache narinfo exceeds 1 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(String::from_utf8(body)?))
}

fn metadata_matches(body: &str, store_path: &Path, nar_hash: &Hash) -> anyhow::Result<bool> {
    Ok(matching_url(body, store_path, nar_hash)?.is_some())
}

pub(super) fn matching_url<'a>(
    body: &'a str,
    store_path: &Path,
    nar_hash: &Hash,
) -> anyhow::Result<Option<&'a str>> {
    let mut upstream_path = None;
    let mut upstream_hash = None;
    let mut url = None;
    for line in body.lines().filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').context("Malformed cache narinfo")?;
        let value = value.trim();
        let field = match key {
            "StorePath" => &mut upstream_path,
            "NarHash" => &mut upstream_hash,
            "URL" => &mut url,
            _ => continue,
        };
        if field.replace(value).is_some() {
            return Err(anyhow!("Duplicate {key} in cache narinfo"));
        }
    }
    let upstream_path = upstream_path.context("Cache narinfo lacks StorePath")?;
    if Path::new(upstream_path) != store_path {
        return Ok(None);
    }
    let upstream_hash = Hash::from_typed(upstream_hash.context("Cache narinfo lacks NarHash")?)?;
    if upstream_hash != *nar_hash {
        return Ok(None);
    }
    let url = url
        .filter(|url| !url.is_empty())
        .context("Cache narinfo lacks a download URL")?;
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{http::StatusCode, response::Redirect, routing::get, Router};
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::*;

    const STORE_PATH: &str = "/nix/store/00000000000000000000000000000000-fixture";
    const NARINFO: &str = "StorePath: /nix/store/00000000000000000000000000000000-fixture\nURL: nar/fixture.nar.zstd\nCompression: zstd\nNarHash: sha256:0000000000000000000000000000000000000000000000000000\nNarSize: 0\nReferences: \nSig: fixture-1:first\nSig: fixture-2:second\n";

    struct Server {
        url: String,
        requests: Arc<AtomicUsize>,
        task: JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn server(status: StatusCode, body: String) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let calls = requests.clone();
        let app = Router::new()
            .route(
                "/cache/00000000000000000000000000000000.narinfo",
                get(move || {
                    let calls = calls.clone();
                    let body = body.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (status, body)
                    }
                }),
            )
            .route(
                "/redirect/00000000000000000000000000000000.narinfo",
                get(|| async {
                    Redirect::temporary("/cache/00000000000000000000000000000000.narinfo")
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Server {
            url,
            requests,
            task,
        }
    }

    #[test]
    fn path_mismatch_does_not_parse_the_nar_hash() {
        let body = NARINFO
            .replace("-fixture", "-other")
            .replace("NarHash:", "MissingHash:");
        assert!(
            matching_url(&body, Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn skips_paths_with_matching_upstream_metadata() {
        let server = server(StatusCode::OK, NARINFO.to_owned()).await;
        let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
        assert!(
            upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn normalizes_trailing_slashes_and_follows_redirects() {
        for base in ["cache/", "redirect/"] {
            let server = server(StatusCode::OK, NARINFO.to_owned()).await;
            let mut upstream = UpstreamCache::new(Some(format!("{}/{base}", server.url))).unwrap();
            assert!(
                upstream
                    .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                    .await
            );
            assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn accepts_equivalent_hexadecimal_nar_hashes() {
        let server = server(
            StatusCode::OK,
            NARINFO.replace(
                "sha256:0000000000000000000000000000000000000000000000000000",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            ),
        )
        .await;
        let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
        assert!(
            upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
    }

    #[tokio::test]
    async fn uploads_when_upstream_metadata_is_absent_or_unavailable() {
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let server = server(status, NARINFO.to_owned()).await;
            let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
            assert!(
                !upstream
                    .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                    .await
            );
        }
    }

    #[tokio::test]
    async fn uploads_when_identity_mismatches_or_metadata_is_malformed() {
        for body in [
            NARINFO.replace("-fixture", "-other"),
            NARINFO.replace(
                "0000000000000000000000000000000000000000000000000000",
                "1111111111111111111111111111111111111111111111111111",
            ),
            NARINFO.replace("NarHash:", "MissingHash:"),
            NARINFO.replace("StorePath:", "MissingPath:"),
            NARINFO.replace("URL: nar/fixture.nar.zstd", "URL: "),
            NARINFO.replace(
                "sha256:0000000000000000000000000000000000000000000000000000",
                "sha256:invalid",
            ),
            format!("{NARINFO}StorePath: {STORE_PATH}\n"),
            format!(
                "{NARINFO}NarHash: sha256:0000000000000000000000000000000000000000000000000000\n"
            ),
            "not narinfo".to_owned(),
        ] {
            let server = server(StatusCode::OK, body).await;
            let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
            assert!(
                !upstream
                    .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                    .await
            );
        }
    }

    #[tokio::test]
    async fn stops_looking_up_paths_after_upstream_failure() {
        let server = server(StatusCode::SERVICE_UNAVAILABLE, "unavailable".to_owned()).await;
        let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
        for _ in 0..2 {
            assert!(
                !upstream
                    .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                    .await
            );
        }
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn keeps_filtering_after_a_missing_path() {
        let server = server(StatusCode::OK, NARINFO.to_owned()).await;
        let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
        assert!(
            !upstream
                .contains(
                    Path::new("/nix/store/11111111111111111111111111111111-missing"),
                    &Hash::Sha256([0; 32])
                )
                .await
        );
        assert!(
            upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
    }

    #[tokio::test]
    async fn uploads_when_metadata_exceeds_the_size_limit() {
        let body = format!("{NARINFO}Padding: {}\n", "x".repeat(1024 * 1024));
        let server = server(StatusCode::OK, body).await;
        let mut upstream = UpstreamCache::new(Some(format!("{}/cache", server.url))).unwrap();
        assert!(
            !upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
    }

    #[tokio::test]
    async fn uploads_without_an_upstream_or_when_connection_fails() {
        let mut upstream = UpstreamCache::new(None).unwrap();
        assert!(
            !upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let mut upstream = UpstreamCache::new(Some(url)).unwrap();
        assert!(
            !upstream
                .contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32]))
                .await
        );
    }

    #[tokio::test]
    async fn times_out_stalled_upstream_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(get(|| async {
            std::future::pending::<()>().await;
            ""
        }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut upstream = UpstreamCache::new(Some(url)).unwrap();
        let found = tokio::time::timeout(
            Duration::from_secs(10),
            upstream.contains(Path::new(STORE_PATH), &Hash::Sha256([0; 32])),
        )
        .await
        .expect("upstream lookup must not hang the upload worker");
        assert!(!found);
        task.abort();
    }
}
