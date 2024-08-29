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
mod env;
mod error;
mod flakehub;
mod gha;
mod pbh;
mod telemetry;
mod util;

use std::collections::HashSet;
use std::fs::{self, create_dir_all};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ::attic::nix_store::NixStore;
use anyhow::{anyhow, Context, Result};
use axum::{extract::Extension, routing::get, Router};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::{oneshot, Mutex, RwLock};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use gha_cache::Credentials;

const DETERMINATE_STATE_DIR: &str = "/nix/var/determinate";
const DETERMINATE_NIXD_SOCKET_NAME: &str = "determinate-nixd.socket";

// TODO(colemickens): refactor, move with other UDS stuff (or all PBH stuff) to new file
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "kebab-case")]
pub struct BuiltPathResponseEventV1 {
    pub drv: PathBuf,
    pub outputs: Vec<PathBuf>,
}

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

    #[arg(long)]
    flakehub_flake_name: Option<String>,

    /// The location of `nix.conf`.
    #[arg(long)]
    nix_conf: PathBuf,

    /// Whether to use the GHA cache.
    #[arg(long)]
    use_gha_cache: bool,

    /// Whether to use the FlakeHub binary cache.
    #[arg(long)]
    use_flakehub: bool,

    /// URL to which to post startup notification.
    #[arg(long)]
    startup_notification_url: Option<reqwest::Url>,

    /// File to write to when indicating startup.
    #[arg(long)]
    startup_notification_file: Option<PathBuf>,

    /// Whether or not to diff the store before and after Magic Nix Cache runs
    #[arg(long, default_value_t = false)]
    diff_store: bool,
}

impl Args {
    fn validate(&self, environment: env::Environment) -> Result<(), error::Error> {
        if environment.is_gitlab_ci() && self.use_gha_cache {
            return Err(error::Error::Config(String::from(
                "the --use-gha-cache flag should not be applied in GitLab CI",
            )));
        }

        if environment.is_gitlab_ci() && !self.use_flakehub {
            return Err(error::Error::Config(String::from(
                "you must set --use-flakehub in GitLab CI",
            )));
        }

        Ok(())
    }
}

/// The global server state.
struct StateInner {
    /// State for uploading to the GHA cache.
    gha_cache: Option<gha::GhaCache>,

    /// The upstream cache.
    upstream: Option<String>,

    /// The sender half of the oneshot channel to trigger a shutdown.
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>,

    /// Set of store path hashes that are not present in GHAC.
    narinfo_negative_cache: Arc<RwLock<HashSet<String>>>,

    /// Metrics for sending to perf at shutdown
    metrics: Arc<telemetry::TelemetryReport>,

    /// Connection to the local Nix store.
    store: Arc<NixStore>,

    /// FlakeHub cache state.
    flakehub_state: RwLock<Option<flakehub::State>>,

    /// Where all of tracing will log to when GitHub Actions is run in debug mode
    logfile: Option<PathBuf>,

    /// The paths in the Nix store when Magic Nix Cache started, if store diffing is enabled.
    original_paths: Option<Mutex<HashSet<PathBuf>>>,
}

async fn main_cli() -> Result<()> {
    let guard = init_logging()?;
    let _tracing_guard = guard.appender_guard;

    let args = Args::parse();
    let environment = env::Environment::determine();
    tracing::debug!("Running in {}", environment.to_string());
    args.validate(environment)?;

    let metrics = Arc::new(telemetry::TelemetryReport::new());

    let dnixd_uds_socket_dir: &Path = Path::new(&DETERMINATE_STATE_DIR);
    let dnixd_uds_socket_path = dnixd_uds_socket_dir.join(DETERMINATE_NIXD_SOCKET_NAME);
    let dnixd_available = dnixd_uds_socket_path.exists();

    // NOTE: we expect this to point to a user nix.conf
    // we always open/append to it to be able to append the extra-substituter for github-actions cache
    // but we don't write to it for initializing flakehub_cache unless dnixd is unavailable
    if let Some(parent) = Path::new(&args.nix_conf).parent() {
        create_dir_all(parent).with_context(|| "Creating parent directories of nix.conf")?;
    }
    let mut nix_conf = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.nix_conf)
        .with_context(|| "Creating nix.conf")?;

    let store = Arc::new(NixStore::connect()?);

    let narinfo_negative_cache = Arc::new(RwLock::new(HashSet::new()));

    let flakehub_state = if args.use_flakehub {
        let flakehub_cache_server = args
            .flakehub_cache_server
            .ok_or_else(|| anyhow!("--flakehub-cache-server is required"))?;
        let flakehub_api_server_netrc = args
            .flakehub_api_server_netrc
            .ok_or_else(|| anyhow!("--flakehub-api-server-netrc is required"))?;
        let flakehub_flake_name = args.flakehub_flake_name;

        match flakehub::init_cache(
            environment,
            &args
                .flakehub_api_server
                .ok_or_else(|| anyhow!("--flakehub-api-server is required"))?,
            &flakehub_api_server_netrc,
            &flakehub_cache_server,
            flakehub_flake_name,
            store.clone(),
        )
        .await
        {
            Ok(state) => {
                if !dnixd_available {
                    nix_conf
                        .write_all(
                            format!(
                                "extra-substituters = {}?trusted=1\nnetrc-file = {}\n",
                                &flakehub_cache_server,
                                flakehub_api_server_netrc.display()
                            )
                            .as_bytes(),
                        )
                        .with_context(|| "Writing to nix.conf")?;
                }

                tracing::info!("FlakeHub cache is enabled.");
                Some(state)
            }
            Err(err) => {
                tracing::debug!("FlakeHub cache initialization failed: {}", err);
                None
            }
        }
    } else {
        tracing::info!(
            "FlakeHub cache is disabled, as the `use-flakehub` setting is set to `false`."
        );
        None
    };

    let gha_cache = if args.use_gha_cache {
        let credentials = if let Some(credentials_file) = &args.credentials_file {
            tracing::info!("Loading credentials from {:?}", credentials_file);
            let bytes = fs::read(credentials_file).with_context(|| {
                format!(
                    "Failed to read credentials file '{}'",
                    credentials_file.display()
                )
            })?;

            serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "Failed to deserialize credentials file '{}'",
                    credentials_file.display()
                )
            })?
        } else {
            tracing::info!("Loading credentials from environment");
            Credentials::load_from_env()
                .with_context(|| "Failed to load credentials from environment (see README.md)")?
        };

        let gha_cache = gha::GhaCache::new(
            credentials,
            args.cache_version,
            store.clone(),
            metrics.clone(),
            narinfo_negative_cache.clone(),
        )
        .with_context(|| "Failed to initialize GitHub Actions Cache API")?;

        nix_conf
            .write_all(format!("extra-substituters = http://{}?trusted=1&compression=zstd&parallel-compression=true&priority=1\n", args.listen).as_bytes())
            .with_context(|| "Writing to nix.conf")?;

        tracing::info!("Native GitHub Action cache is enabled.");
        Some(gha_cache)
    } else {
        if environment.is_github_actions() {
            tracing::info!("Native GitHub Action cache is disabled.");
        }

        None
    };

    let diagnostic_endpoint = match args.diagnostic_endpoint.as_str() {
        "" => {
            tracing::info!("Diagnostics disabled.");
            None
        }
        url => Some(url),
    };

    let (shutdown_sender, shutdown_receiver) = oneshot::channel();

    let original_paths = args.diff_store.then_some(Mutex::new(HashSet::new()));
    let state = Arc::new(StateInner {
        gha_cache,
        upstream: args.upstream.clone(),
        shutdown_sender: Mutex::new(Some(shutdown_sender)),
        narinfo_negative_cache,
        metrics,
        store,
        flakehub_state: RwLock::new(flakehub_state),
        logfile: guard.logfile,
        original_paths,
    });

    if dnixd_available {
        tracing::info!("Subscribing to Determinate Nixd build events.");
        crate::pbh::subscribe_uds_post_build_hook(dnixd_uds_socket_path, state.clone()).await?;
    } else {
        tracing::info!("Patching nix.conf to use a post-build-hook.");
        crate::pbh::setup_legacy_post_build_hook(&args.listen, &mut nix_conf).await?;
    }

    drop(nix_conf);

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

    // Notify of startup via HTTP
    if let Some(startup_notification_url) = args.startup_notification_url {
        tracing::debug!("Startup notification via HTTP POST to {startup_notification_url}");

        let response = reqwest::Client::new()
            .post(startup_notification_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await;
        match response {
            Ok(response) => {
                if !response.status().is_success() {
                    Err(anyhow!(
                        "Startup notification returned an error: {}\n{}",
                        response.status(),
                        response
                            .text()
                            .await
                            .unwrap_or_else(|_| "<no response text>".to_owned())
                    ))?;
                }
            }
            err @ Err(_) => {
                err.with_context(|| "Startup notification failed")?;
            }
        }
    }

    // Notify of startup by writing "1" to the specified file
    if let Some(startup_notification_file_path) = args.startup_notification_file {
        let file_contents: &[u8] = b"1";

        tracing::debug!("Startup notification via file at {startup_notification_file_path:?}");

        if let Some(parent_dir) = startup_notification_file_path.parent() {
            tokio::fs::create_dir_all(parent_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to create parent directory for startup notification file path: {}",
                        startup_notification_file_path.display()
                    )
                })?;
        }
        let mut notification_file = File::create(&startup_notification_file_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create startup notification file to path: {}",
                    startup_notification_file_path.display()
                )
            })?;
        notification_file
            .write_all(file_contents)
            .await
            .with_context(|| {
                format!(
                    "failed to write startup notification file to path: {}",
                    startup_notification_file_path.display()
                )
            })?;

        tracing::debug!("Created startup notification file at {startup_notification_file_path:?}");
    }

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    let ret = axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async move {
            shutdown_receiver.await.ok();
            tracing::info!("Shutting down");
        })
        .await;

    // Notify diagnostics endpoint
    if let Some(diagnostic_endpoint) = diagnostic_endpoint {
        state.metrics.send(diagnostic_endpoint).await;
    }

    ret?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::var("OUT_PATHS") {
        Ok(out_paths) => pbh::handle_legacy_post_build_hook(&out_paths).await,
        Err(_) => main_cli().await,
    }
}

pub(crate) fn debug_logfile() -> PathBuf {
    std::env::temp_dir().join("magic-nix-cache-tracing.log")
}

pub struct LogGuard {
    appender_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
    logfile: Option<PathBuf>,
}

fn init_logging() -> Result<LogGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        #[cfg(debug_assertions)]
        return EnvFilter::new("info")
            .add_directive("magic_nix_cache=debug".parse().unwrap())
            .add_directive("gha_cache=debug".parse().unwrap());

        #[cfg(not(debug_assertions))]
        return EnvFilter::new("info");
    });

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .pretty();

    let (guard, file_layer) = match std::env::var("RUNNER_DEBUG") {
        Ok(val) if val == "1" => {
            let logfile = debug_logfile();
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&logfile)?;
            let (nonblocking, guard) = tracing_appender::non_blocking(file);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(nonblocking)
                .pretty();

            (
                LogGuard {
                    appender_guard: Some(guard),
                    logfile: Some(logfile),
                },
                Some(file_layer),
            )
        }
        _ => (
            LogGuard {
                appender_guard: None,
                logfile: None,
            },
            None,
        ),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    Ok(guard)
}

#[cfg(debug_assertions)]
async fn dump_api_stats(
    Extension(state): Extension<State>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(gha_cache) = &state.gha_cache {
        gha_cache.api.dump_stats();
    }
    next.run(request).await
}

async fn root() -> &'static str {
    "cache the world 🚀"
}
