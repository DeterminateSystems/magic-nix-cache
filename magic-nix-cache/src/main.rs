#![deny(
    asm_sub_register,
    deprecated,
    missing_abi,
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
mod binary_cache;
mod error;
mod flakehub;
mod telemetry;
mod util;

use std::collections::HashSet;
use std::fs::{self, create_dir_all, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ::attic::nix_store::NixStore;
use axum::{extract::Extension, routing::get, Router};
use clap::Parser;
use tempfile::NamedTempFile;
use tokio::sync::{oneshot, Mutex, RwLock};
use tracing_subscriber::filter::EnvFilter;

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

    /// Diagnostic endpoint to send diagnostics and performance data.
    ///
    /// Set it to an empty string to disable reporting.
    /// See the README for details.
    #[arg(
        long,
        default_value = "https://install.determinate.systems/magic-nix-cache/perf"
    )]
    diagnostic_endpoint: String,

    /// The FlakeHub API server.
    #[arg(long)]
    flakehub_api_server: Option<reqwest::Url>,

    /// The path of the `netrc` file that contains the FlakeHub JWT token.
    #[arg(long)]
    flakehub_api_server_netrc: Option<PathBuf>,

    /// The FlakeHub binary cache server.
    #[arg(long)]
    flakehub_cache_server: Option<reqwest::Url>,

    /// The location of `nix.conf`.
    #[arg(long)]
    nix_conf: PathBuf,

    /// Whether to use the GHA cache.
    #[arg(long)]
    use_gha_cache: bool,

    /// Whether to use the FlakeHub binary cache.
    #[arg(long)]
    use_flakehub: bool,

    /// File descriptor on which to send startup notification.
    #[arg(long)]
    notify_fd: Option<i32>,
}

/// The global server state.
struct StateInner {
    /// The GitHub Actions Cache API.
    api: Option<Api>,

    /// The upstream cache.
    upstream: Option<String>,

    /// The sender half of the oneshot channel to trigger a shutdown.
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>,

    /// List of store paths originally present.
    original_paths: Mutex<HashSet<PathBuf>>,

    /// Set of store path hashes that are not present in GHAC.
    narinfo_nagative_cache: RwLock<HashSet<String>>,

    /// Endpoint of ourselves.
    ///
    /// This is used by our Action API to invoke `nix copy` to upload new paths.
    self_endpoint: SocketAddr,

    /// Metrics for sending to perf at shutdown
    metrics: telemetry::TelemetryReport,

    /// Connection to the local Nix store.
    store: Arc<NixStore>,

    /// FlakeHub cache state.
    flakehub_state: RwLock<Option<flakehub::State>>,
}

async fn main_cli() {
    init_logging();

    let args = Args::parse();

    create_dir_all(Path::new(&args.nix_conf).parent().unwrap())
        .expect("Creating parent directories of nix.conf");

    let mut nix_conf = OpenOptions::new()
        .create(true)
        .append(true)
        .open(args.nix_conf)
        .expect("Opening nix.conf");

    let store = Arc::new(NixStore::connect().expect("Connecting to the Nix store"));

    let flakehub_state = if args.use_flakehub {
        let flakehub_cache_server = args
            .flakehub_cache_server
            .expect("--flakehub-cache-server is required");
        let flakehub_api_server_netrc = args
            .flakehub_api_server_netrc
            .expect("--flakehub-api-server-netrc is required");

        match flakehub::init_cache(
            &args
                .flakehub_api_server
                .expect("--flakehub-api-server is required"),
            &flakehub_api_server_netrc,
            &flakehub_cache_server,
            store.clone(),
        )
        .await
        {
            Ok(state) => {
                nix_conf
                    .write_all(
                        format!(
                            "extra-substituters = {}?trusted=1\nnetrc-file = {}\n",
                            &flakehub_cache_server,
                            flakehub_api_server_netrc.display()
                        )
                        .as_bytes(),
                    )
                    .expect("Writing to nix.conf");

                tracing::info!("FlakeHub cache is enabled.");
                Some(state)
            }
            Err(err) => {
                tracing::error!("FlakeHub cache initialization failed: {}", err);
                None
            }
        }
    } else {
        tracing::info!("FlakeHub cache is disabled.");
        None
    };

    let api = if args.use_gha_cache {
        let credentials = if let Some(credentials_file) = &args.credentials_file {
            tracing::info!("Loading credentials from {:?}", credentials_file);
            let bytes = fs::read(credentials_file).expect("Failed to read credentials file");

            serde_json::from_slice(&bytes).expect("Failed to deserialize credentials file")
        } else {
            tracing::info!("Loading credentials from environment");
            Credentials::load_from_env()
                .expect("Failed to load credentials from environment (see README.md)")
        };

        let mut api = Api::new(credentials).expect("Failed to initialize GitHub Actions Cache API");

        if let Some(cache_version) = &args.cache_version {
            api.mutate_version(cache_version.as_bytes());
        }

        nix_conf
            .write_all(format!("extra-substituters = http://{}?trusted=1&compression=zstd&parallel-compression=true&priority=1\n", args.listen).as_bytes())
            .expect("Writing to nix.conf");

        tracing::info!("GitHub Action cache is enabled.");
        Some(api)
    } else {
        tracing::info!("GitHub Action cache is disabled.");
        None
    };

    /* Write the post-build hook script. Note that the shell script
     * ignores errors, to avoid the Nix build from failing. */
    let post_build_hook_script = {
        let mut file = NamedTempFile::with_prefix("magic-nix-cache-build-hook-")
            .expect("Creating a temporary file");
        file.write_all(
            format!(
                "#! /bin/sh\nexec env RUST_BACKTRACE=full {} --server {}\n",
                std::env::current_exe()
                    .expect("Getting the path of magic-nix-cache")
                    .display(),
                args.listen
            )
            .as_bytes(),
        )
        .expect("Writing the post-build hook");
        file.keep().unwrap().1
    };

    fs::set_permissions(&post_build_hook_script, fs::Permissions::from_mode(0o755)).unwrap();

    /* Update nix.conf. */
    nix_conf
        .write_all(
            format!(
                "fallback = true\npost-build-hook = {}\n",
                post_build_hook_script.display()
            )
            .as_bytes(),
        )
        .expect("Writing to nix.conf");

    drop(nix_conf);

    let diagnostic_endpoint = match args.diagnostic_endpoint.as_str() {
        "" => {
            tracing::info!("Diagnostics disabled.");
            None
        }
        url => Some(url),
    };

    let (shutdown_sender, shutdown_receiver) = oneshot::channel();

    let state = Arc::new(StateInner {
        api,
        upstream: args.upstream.clone(),
        shutdown_sender: Mutex::new(Some(shutdown_sender)),
        original_paths: Mutex::new(HashSet::new()),
        narinfo_nagative_cache: RwLock::new(HashSet::new()),
        self_endpoint: args.listen.to_owned(),
        metrics: telemetry::TelemetryReport::new(),
        store,
        flakehub_state: RwLock::new(flakehub_state),
    });

    let app = Router::new()
        .route("/", get(root))
        .merge(api::get_router())
        .merge(binary_cache::get_router());

    #[cfg(debug_assertions)]
    let app = app
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(dump_api_stats));

    let app = app.layer(Extension(state.clone()));

    tracing::info!("Listening on {}", args.listen);

    if let Some(notify_fd) = args.notify_fd {
        let mut f = unsafe { std::fs::File::from_raw_fd(notify_fd) };
        writeln!(&mut f, "INIT").unwrap();
    }

    let ret = axum::Server::bind(&args.listen)
        .serve(app.into_make_service())
        .with_graceful_shutdown(async move {
            shutdown_receiver.await.ok();
            tracing::info!("Shutting down");
        })
        .await;

    if let Some(diagnostic_endpoint) = diagnostic_endpoint {
        state.metrics.send(diagnostic_endpoint).await;
    }

    ret.unwrap()
}

async fn post_build_hook(out_paths: &str) {
    #[derive(Parser, Debug)]
    struct Args {
        /// `magic-nix-cache` daemon to connect to.
        #[arg(short = 'l', long, default_value = "127.0.0.1:3000")]
        server: SocketAddr,
    }

    let args = Args::parse();

    let store_paths: Vec<_> = out_paths.lines().map(str::to_owned).collect();

    let request = api::EnqueuePathsRequest { store_paths };

    let response = reqwest::Client::new()
        .post(format!("http://{}/api/enqueue-paths", &args.server))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&request).unwrap())
        .send()
        .await
        .unwrap();

    if !response.status().is_success() {
        eprintln!(
            "magic-nix-cache server failed to enqueue the push request: {}",
            response.status()
        );
    } else {
        response.json::<api::EnqueuePathsResponse>().await.unwrap();
    }
}

#[tokio::main]
async fn main() {
    match std::env::var("OUT_PATHS") {
        Ok(out_paths) => post_build_hook(&out_paths).await,
        Err(_) => main_cli().await,
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        #[cfg(debug_assertions)]
        return EnvFilter::new("info")
            .add_directive("magic_nix_cache=debug".parse().unwrap())
            .add_directive("gha_cache=debug".parse().unwrap());

        #[cfg(not(debug_assertions))]
        return EnvFilter::new("info");
    });

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .pretty()
        .with_env_filter(filter)
        .init();
}

#[cfg(debug_assertions)]
async fn dump_api_stats<B>(
    Extension(state): Extension<State>,
    request: axum::http::Request<B>,
    next: axum::middleware::Next<B>,
) -> axum::response::Response {
    if let Some(api) = &state.api {
        api.dump_stats();
    }
    next.run(request).await
}

async fn root() -> &'static str {
    "cache the world 🚀"
}
