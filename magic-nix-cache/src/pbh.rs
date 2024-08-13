use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use anyhow::anyhow;
use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use futures::StreamExt as _;
use http_body_util::BodyExt as _;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use tempfile::NamedTempFile;
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::BuiltPathResponseEventV1;
use crate::State;

pub async fn subscribe_uds_post_build_hook(
    dnixd_uds_socket_path: PathBuf,
    state: State,
) -> Result<()> {
    tokio::spawn(async move {
        let dnixd_uds_socket_path = &dnixd_uds_socket_path;
        loop {
            let Ok(socket_conn) = UnixStream::connect(dnixd_uds_socket_path).await else {
                tracing::error!("built-paths: failed to connect to determinate-nixd's socket");
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            };
            let stream = TokioIo::new(socket_conn);
            let executor: TokioExecutor = TokioExecutor::new();

            let sender_conn = hyper::client::conn::http2::handshake(executor, stream).await;

            let Ok((mut sender, conn)) = sender_conn else {
                tracing::error!("built-paths: failed to http2 handshake");
                continue;
            };

            // NOTE(colemickens): for now we just drop the joinhandle and let it keep running
            let _join_handle = tokio::task::spawn(async move {
                if let Err(err) = conn.await {
                    tracing::error!("Connection failed: {:?}", err);
                }
            });

            let request = http::Request::builder()
                .method(http::Method::GET)
                .uri("http://localhost/events")
                .body(axum::body::Body::empty());
            let Ok(request) = request else {
                tracing::error!("built-paths: failed to create request to subscribe");
                continue;
            };

            let response = sender.send_request(request).await;
            let Ok(response) = response else {
                tracing::error!("buit-paths: failed to send subscription request");
                continue;
            };
            let mut data = response.into_data_stream();

            while let Some(event_str) = data.next().await {
                let event_str = match event_str {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::error!("built-paths: error while receiving: {}", e);
                        break;
                    }
                };

                let Some(event_str) = event_str.strip_prefix("data: ".as_bytes()) else {
                    tracing::debug!("built-paths subscription: ignoring non-data frame");
                    continue;
                };
                let Ok(event): core::result::Result<BuiltPathResponseEventV1, _> =
                    serde_json::from_slice(event_str)
                else {
                    tracing::error!(
                        "failed to decode built-path response as BuiltPathResponseEventV1"
                    );
                    continue;
                };

                // TODO(colemickens): error handling:::
                let maybe_store_paths = event
                    .outputs
                    .iter()
                    .map(|path| {
                        state
                            .store
                            .follow_store_path(path)
                            .map_err(|_| anyhow!("ahhhhh"))
                    })
                    .collect::<Result<Vec<_>>>();

                let Ok(store_paths) = maybe_store_paths else {
                    tracing::error!(
                        "built-paths: encountered an error aggregating build store paths"
                    );
                    continue;
                };

                tracing::debug!("about to enqueue paths: {:?}", store_paths);
                if let Err(e) = crate::api::enqueue_paths(&state, store_paths).await {
                    tracing::error!(
                        "built-paths: failed to enqueue paths for drv ({}): {}",
                        event.drv.display(),
                        e
                    );
                    continue;
                }
            }
        }
    });

    Ok(())
}

pub async fn setup_legacy_post_build_hook(
    listen: &SocketAddr,
    nix_conf: &mut std::fs::File,
) -> Result<()> {
    /* Write the post-build hook script. Note that the shell script
     * ignores errors, to avoid the Nix build from failing. */
    let post_build_hook_script = {
        let mut file = NamedTempFile::with_prefix("magic-nix-cache-build-hook-")
            .with_context(|| "Creating a temporary file for the post-build hook")?;
        file.write_all(
            format!(
                // NOTE(cole-h): We want to exit 0 even if the hook failed, otherwise it'll fail the
                // build itself
                "#! /bin/sh\nRUST_LOG=trace RUST_BACKTRACE=full {} --server {} || :\n",
                std::env::current_exe()
                    .with_context(|| "Getting the path of magic-nix-cache")?
                    .display(),
                listen
            )
            .as_bytes(),
        )
        .with_context(|| "Writing the post-build hook")?;
        let path = file
            .keep()
            .with_context(|| "Keeping the post-build hook")?
            .1;

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| "Setting permissions on the post-build hook")?;

        /* Copy the script to the Nix store so we know for sure that
         * it's accessible to the Nix daemon, which might have a
         * different /tmp from us. */
        let res = Command::new("nix")
            .args([
                "--extra-experimental-features",
                "nix-command",
                "store",
                "add-path",
                &path.display().to_string(),
            ])
            .output()
            .await?;
        if res.status.success() {
            tokio::fs::remove_file(path).await?;
            PathBuf::from(String::from_utf8_lossy(&res.stdout).trim())
        } else {
            path
        }
    };

    /* Update nix.conf. */
    nix_conf
        .write_all(
            format!(
                "fallback = true\npost-build-hook = {}\n",
                post_build_hook_script.display()
            )
            .as_bytes(),
        )
        .with_context(|| "Writing to nix.conf")?;

    Ok(())
}

pub async fn handle_legacy_post_build_hook(out_paths: &str) -> Result<()> {
    #[derive(Parser, Debug)]
    struct Args {
        /// `magic-nix-cache` daemon to connect to.
        #[arg(short = 'l', long, default_value = "127.0.0.1:3000")]
        server: SocketAddr,
    }

    let args = Args::parse();

    let store_paths: Vec<_> = out_paths
        .split_whitespace()
        .map(|s| s.trim().to_owned())
        .collect();

    let request = crate::api::EnqueuePathsRequest { store_paths };

    let response = reqwest::Client::new()
        .post(format!("http://{}/api/enqueue-paths", &args.server))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_string(&request)
                .with_context(|| "Decoding the response from the magic-nix-cache server")?,
        )
        .send()
        .await;

    match response {
        Ok(response) if !response.status().is_success() => Err(anyhow!(
            "magic-nix-cache server failed to enqueue the push request: {}\n{}",
            response.status(),
            response
                .text()
                .await
                .unwrap_or_else(|_| "<no response text>".to_owned()),
        ))?,
        Ok(response) => response
            .json::<crate::api::EnqueuePathsResponse>()
            .await
            .with_context(|| "magic-nix-cache-server didn't return a valid response")?,
        Err(err) => {
            Err(err).with_context(|| "magic-nix-cache server failed to send the enqueue request")?
        }
    };

    Ok(())
}
