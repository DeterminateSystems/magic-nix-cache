//! Binary Cache API.

use std::io;

use axum::{
    extract::{BodyStream, Extension, Path},
    response::Redirect,
    routing::{get, put},
    Router,
};
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;

use super::State;
use crate::error::{Error, Result};

pub fn get_router() -> Router {
    Router::new()
        .route("/nix-cache-info", get(get_nix_cache_info))
        // .narinfo
        .route("/:path", get(get_narinfo))
        .route("/:path", put(put_narinfo))
        // .nar
        .route("/nar/:path", get(get_nar))
        .route("/nar/:path", put(put_nar))
}

async fn get_nix_cache_info() -> &'static str {
    // TODO: Make StoreDir configurable
    r#"WantMassQuery: 1
StoreDir: /nix/store
Priority: 41
"#
}

async fn get_narinfo(
    Extension(state): Extension<State>,
    Path(path): Path<String>,
) -> Result<Redirect> {
    let components: Vec<&str> = path.splitn(2, '.').collect();

    if components.len() != 2 {
        return Err(Error::NotFound);
    }

    if components[1] != "narinfo" {
        return Err(Error::NotFound);
    }

    let store_path_hash = components[0].to_string();
    let key = format!("{}.narinfo", store_path_hash);

    if state
        .narinfo_nagative_cache
        .read()
        .await
        .contains(&store_path_hash)
    {
        state.metrics.narinfos_sent_upstream.incr();
        state.metrics.narinfos_negative_cache_hits.incr();
        return pull_through(&state, &path);
    }

    if let Some(api) = &state.api {
        if let Some(url) = api.get_file_url(&[&key]).await? {
            state.metrics.narinfos_served.incr();
            return Ok(Redirect::temporary(&url));
        }
    }

    let mut negative_cache = state.narinfo_nagative_cache.write().await;
    negative_cache.insert(store_path_hash);

    state.metrics.narinfos_sent_upstream.incr();
    state.metrics.narinfos_negative_cache_misses.incr();
    pull_through(&state, &path)
}
async fn put_narinfo(
    Extension(state): Extension<State>,
    Path(path): Path<String>,
    body: BodyStream,
) -> Result<()> {
    let components: Vec<&str> = path.splitn(2, '.').collect();

    if components.len() != 2 {
        return Err(Error::BadRequest);
    }

    if components[1] != "narinfo" {
        return Err(Error::BadRequest);
    }

    let api = state.api.as_ref().ok_or(Error::GHADisabled)?;

    let store_path_hash = components[0].to_string();
    let key = format!("{}.narinfo", store_path_hash);
    let allocation = api.allocate_file_with_random_suffix(&key).await?;
    let stream = StreamReader::new(
        body.map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))),
    );
    api.upload_file(allocation, stream).await?;
    state.metrics.narinfos_uploaded.incr();

    state
        .narinfo_nagative_cache
        .write()
        .await
        .remove(&store_path_hash);

    Ok(())
}

async fn get_nar(Extension(state): Extension<State>, Path(path): Path<String>) -> Result<Redirect> {
    if let Some(url) = state
        .api
        .as_ref()
        .ok_or(Error::GHADisabled)?
        .get_file_url(&[&path])
        .await?
    {
        state.metrics.nars_served.incr();
        return Ok(Redirect::temporary(&url));
    }

    if let Some(upstream) = &state.upstream {
        state.metrics.nars_sent_upstream.incr();
        Ok(Redirect::temporary(&format!("{}/nar/{}", upstream, path)))
    } else {
        Err(Error::NotFound)
    }
}
async fn put_nar(
    Extension(state): Extension<State>,
    Path(path): Path<String>,
    body: BodyStream,
) -> Result<()> {
    let api = state.api.as_ref().ok_or(Error::GHADisabled)?;

    let allocation = api.allocate_file_with_random_suffix(&path).await?;
    let stream = StreamReader::new(
        body.map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))),
    );
    api.upload_file(allocation, stream).await?;
    state.metrics.nars_uploaded.incr();

    Ok(())
}

fn pull_through(state: &State, path: &str) -> Result<Redirect> {
    if let Some(upstream) = &state.upstream {
        Ok(Redirect::temporary(&format!("{}/{}", upstream, path)))
    } else {
        Err(Error::NotFound)
    }
}
