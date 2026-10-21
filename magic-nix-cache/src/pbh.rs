use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use eventsource_stream::Eventsource as _;
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

        tokio::select! {
            _ = state.shutdown_token.cancelled() => {
            }
            _ = handle_events(Arc::clone(&state), dnixd_uds_socket_path) => {
            }
        }
    });

    Ok(())
}

/// How long to wait before reconnecting to determinate-nixd after a failure.
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// The envelope every determinate-nixd event carries.
///
/// Only `v` and `c` are read here, so that events of other kinds can be
/// skipped without trying to decode them as a built-path event.
#[derive(Debug, serde::Deserialize)]
struct EventEnvelope {
    v: String,
    c: String,
}

async fn handle_events(state: State, dnixd_uds_socket_path: &PathBuf) -> ! {
    loop {
        let Ok(socket_conn) = UnixStream::connect(dnixd_uds_socket_path).await else {
            tracing::error!("built-paths: failed to connect to determinate-nixd's socket");
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };
        let stream = TokioIo::new(socket_conn);
        let executor: TokioExecutor = TokioExecutor::new();

        let sender_conn = hyper::client::conn::http2::handshake(executor, stream).await;

        let Ok((mut sender, conn)) = sender_conn else {
            tracing::error!("built-paths: failed to http2 handshake");
            tokio::time::sleep(RECONNECT_DELAY).await;
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
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };

        let response = match sender.send_request(request).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("built-paths: failed to send subscription request: {:?}", e);
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        if !response.status().is_success() {
            tracing::error!(
                status = %response.status(),
                "built-paths: determinate-nixd rejected the subscription"
            );
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        // The body arrives as a sequence of HTTP/2 data frames. One SSE event
        // can span many frames (a built-path event carries the derivation's
        // provenance, which can be large), so the frames are parsed as a
        // continuous SSE stream rather than one event per frame.
        let mut events = response.into_body().into_data_stream().eventsource();

        loop {
            match events.next().await {
                Some(Ok(event)) => handle_event(&state, &event.data).await,
                Some(Err(e)) => {
                    tracing::error!("built-paths: error while receiving: {}", e);
                    break;
                }
                None => {
                    tracing::warn!("built-paths: event stream ended, reconnecting");
                    break;
                }
            }
        }

        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Handle the data of one SSE event from determinate-nixd.
async fn handle_event(state: &State, data: &str) {
    let envelope: EventEnvelope = match serde_json::from_str(data) {
        Ok(envelope) => envelope,
        Err(e) => {
            tracing::error!(
                data = %data,
                error = %e,
                "built-paths: event is not a determinate-nixd event envelope"
            );
            return;
        }
    };

    if envelope.v != "1" {
        tracing::debug!(version = %envelope.v, "built-paths: ignoring event with unknown version");
        return;
    }

    if envelope.c != "BuiltPathResponseEventV1" {
        tracing::debug!(kind = %envelope.c, "built-paths: ignoring event of another kind");
        return;
    }

    let event: BuiltPathResponseEventV1 = match serde_json::from_str(data) {
        Ok(event) => event,
        Err(e) => {
            tracing::error!(
                data = %data,
                error = %e,
                "built-paths: failed to decode BuiltPathResponseEventV1"
            );
            return;
        }
    };

    let maybe_store_paths = event
        .outputs
        .iter()
        .map(|path| {
            state
                .store
                .follow_store_path(path)
                .map_err(|_| anyhow!("failed to collect store paths"))
        })
        .collect::<Result<Vec<_>>>();

    let Ok(store_paths) = maybe_store_paths else {
        tracing::error!(
            drv = %event.drv.display(),
            "built-paths: encountered an error aggregating build store paths"
        );
        return;
    };

    tracing::debug!(drv = %event.drv.display(), "about to enqueue paths: {:?}", store_paths);
    if let Err(e) = crate::api::enqueue_paths(state, store_paths).await {
        tracing::error!(
            "built-paths: failed to enqueue paths for drv ({}): {}",
            event.drv.display(),
            e
        );
    }
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
            .await
            .with_context(|| {
                format!(
                    "Running nix to add the post-build-hook to the store from {}",
                    path.display()
                )
            })?;
        if res.status.success() {
            tokio::fs::remove_file(&path).await.with_context(|| {
                format!(
                    "Cleaning up the temporary post-build-hook at {}",
                    path.display()
                )
            })?;
            PathBuf::from(String::from_utf8_lossy(&res.stdout).trim())
        } else {
            path
        }
    };

    /* Update nix.conf. */
    nix_conf
        .write_all(format!("post-build-hook = {}\n", post_build_hook_script.display()).as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: a single large SSE event delivered in many small
    /// frames must come out of the SSE parser as one event.
    #[tokio::test]
    async fn large_event_split_across_frames_is_reassembled() {
        let payload = format!(
            "{{\"v\":\"1\",\"c\":\"BuiltPathResponseEventV1\",\"drv\":\"/nix/store/x-example.drv\",\"outputs\":[\"/nix/store/y-example\"],\"provenance\":{{\"meta\":\"{}\"}}}}",
            "v".repeat(200 * 1024)
        );
        let wire = format!(": keep-alive\n\ndata: {payload}\n\n");

        // Deliver the bytes in 16 KiB frames, the HTTP/2 default frame size.
        let frames: Vec<std::io::Result<hyper::body::Bytes>> = wire
            .as_bytes()
            .chunks(16 * 1024)
            .map(|chunk| Ok(hyper::body::Bytes::copy_from_slice(chunk)))
            .collect();
        assert!(frames.len() > 10, "the test payload must span many frames");

        let events: Vec<_> = futures::stream::iter(frames)
            .eventsource()
            .map(|event| event.expect("the stream is valid SSE"))
            .collect()
            .await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, payload);

        let decoded: BuiltPathResponseEventV1 = serde_json::from_str(&events[0].data).unwrap();
        assert_eq!(decoded.outputs.len(), 1);
    }
}
