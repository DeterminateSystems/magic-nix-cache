#![deny(
    asm_sub_register,
    deprecated,
    missing_abi,
    unsafe_code,
    unused_macros,
    unused_must_use,
    unused_unsafe
)]
#![deny(clippy::from_over_into, clippy::needless_question_mark)]
#![cfg_attr(
    not(debug_assertions),
    deny(unused_imports, unused_mut, unused_variables,)
)]

mod error;

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{BodyStream, Extension, Path},
    response::Redirect,
    routing::{get, put},
    Router,
};
use clap::Parser;
use tokio::fs;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;

use error::{Error, Result};
use gha_cache::{Api, Credentials};

type State = Arc<StateInner>;

/// GitHub Actions-powered Nix binary cache
#[derive(Parser, Debug)]
struct Args {
    /// JSON file containing credentials.
    ///
    /// If this is not specified, credentials will be loaded
    /// from the environment.
    #[arg(short = 'c', long)]
    credentials_file: Option<PathBuf>,

    /// Address to listen on.
    ///
    /// FIXME: IPv6
    #[arg(short = 'l', long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,

    /// The cache version.
    ///
    /// Only caches with the same version string are visible.
    /// Using another version string allows you to "bust" the cache.
    #[arg(long)]
    cache_version: Option<String>,
}

/// The global server state.
#[derive(Debug)]
struct StateInner {
    api: Api,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt::init();

    let credentials = if let Some(credentials_file) = &args.credentials_file {
        tracing::info!("Loading credentials from {:?}", credentials_file);
        let bytes = fs::read(credentials_file)
            .await
            .expect("Failed to read credentials file");

        serde_json::from_slice(&bytes).expect("Failed to deserialize credentials file")
    } else {
        tracing::info!("Loading credentials from environment");
        Credentials::load_from_env()
            .expect("Failed to load credentials from environment (see README.md)")
    };

    let mut api = Api::new(credentials).expect("Failed to initialize GitHub Actions Cache API");

    if let Some(cache_version) = args.cache_version {
        api.mutate_version(cache_version.as_bytes());
    }

    let state = Arc::new(StateInner { api });

    let app = Router::new()
        .route("/", get(root))
        .route("/nix-cache-info", get(get_nix_cache_info))
        // .narinfo
        .route("/:path", get(get_narinfo))
        .route("/:path", put(put_narinfo))
        // .nar
        .route("/nar/:path", get(get_nar))
        .route("/nar/:path", put(put_nar));

    #[cfg(debug_assertions)]
    let app = app
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(dump_api_stats));

    let app = app.layer(Extension(state));

    tracing::info!("listening on {}", args.listen);
    axum::Server::bind(&args.listen)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[cfg(debug_assertions)]
async fn dump_api_stats<B>(
    Extension(state): Extension<State>,
    request: axum::http::Request<B>,
    next: axum::middleware::Next<B>,
) -> axum::response::Response {
    state.api.dump_stats();
    next.run(request).await
}

async fn root() -> &'static str {
    "cache the world 🚀"
}

async fn get_nix_cache_info() -> &'static str {
    // TODO: Make StoreDir configurable
    r#"WantMassQuery: 1
StoreDir: /nix/store
Priority: 39
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

    if let Some(url) = state.api.get_file_url(&[&key]).await? {
        return Ok(Redirect::temporary(&url));
    }

    Err(Error::NotFound)
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

    let store_path_hash = components[0].to_string();
    let key = format!("{}.narinfo", store_path_hash);
    let allocation = state.api.allocate_file_with_random_suffix(&key).await?;
    let stream = StreamReader::new(
        body.map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))),
    );
    state.api.upload_file(allocation, stream).await?;

    Ok(())
}

async fn get_nar(Extension(state): Extension<State>, Path(path): Path<String>) -> Result<Redirect> {
    if let Some(url) = state.api.get_file_url(&[&path]).await? {
        return Ok(Redirect::temporary(&url));
    }

    Err(Error::NotFound)
}
async fn put_nar(
    Extension(state): Extension<State>,
    Path(path): Path<String>,
    body: BodyStream,
) -> Result<()> {
    let allocation = state.api.allocate_file_with_random_suffix(&path).await?;
    let stream = StreamReader::new(
        body.map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))),
    );
    state.api.upload_file(allocation, stream).await?;

    Ok(())
}
