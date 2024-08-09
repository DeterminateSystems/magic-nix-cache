use std::net::SocketAddr;

use anyhow::anyhow;
use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;

pub async fn post_build_hook(out_paths: &str) -> Result<()> {
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
