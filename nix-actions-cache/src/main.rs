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

mod api;
mod error;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{extract::Extension, routing::get, Router};
use clap::Parser;
use tokio::fs;
use tracing_subscriber::EnvFilter;

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

    /// The upstream cache.
    ///
    /// Requests for unknown NARs are redirected to this cache
    /// instead.
    #[arg(long)]
    upstream: Option<String>,
}

/// The global server state.
#[derive(Debug)]
struct StateInner {
    api: Api,
    upstream: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    init_logging();

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

    let state = Arc::new(StateInner {
        api,
        upstream: args.upstream,
    });

    let app = Router::new().route("/", get(root)).merge(api::get_router());

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

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        #[cfg(debug_assertions)]
        return EnvFilter::new("gha_cache=debug,nix_action_cache=debug");

        #[cfg(not(debug_assertions))]
        return EnvFilter::default();
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
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
