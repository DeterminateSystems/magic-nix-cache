//! Utilities.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use attic::nix_store::NixStore;
use tokio::{fs, process::Command};

use crate::error::{Error, Result};

/// Returns the list of store paths that are currently present.
pub async fn get_store_paths(store: &NixStore) -> Result<HashSet<PathBuf>> {
    // FIXME: use the Nix API.
    let store_dir = store.store_dir();
    let mut listing = fs::read_dir(store_dir).await?;
    let mut paths = HashSet::new();
    while let Some(entry) = listing.next_entry().await? {
        let file_name = entry.file_name();
        let file_name = Path::new(&file_name);

        if let Some(extension) = file_name.extension() {
            match extension.to_str() {
                None | Some("drv") | Some("lock") | Some("chroot") => {
                    // Malformed or not interesting
                    continue;
                }
                _ => {}
            }
        }

        if let Some(s) = file_name.to_str() {
            // Let's not push any sources
            if s.ends_with("-source") {
                continue;
            }

            // Special paths (so far only `.links`)
            if s.starts_with('.') {
                continue;
            }
        }

        paths.insert(store_dir.join(file_name));
    }
    Ok(paths)
}

/// Uploads a list of store paths to a store URI.
pub async fn upload_paths(mut paths: Vec<PathBuf>, store_uri: &str) -> Result<()> {
    // When the daemon started Nix may not have been installed
    let env_path = Command::new("sh")
        .args(["-lc", "echo $PATH"])
        .output()
        .await?
        .stdout;
    let env_path = String::from_utf8(env_path)
        .map_err(|_| Error::Config("PATH contains invalid UTF-8".to_owned()))?;

    while !paths.is_empty() {
        let mut batch = Vec::new();
        let mut total_len = 0;

        while total_len < 1024 * 1024 {
            if let Some(p) = paths.pop() {
                total_len += p.as_os_str().len() + 1;
                batch.push(p);
            } else {
                break;
            }
        }

        tracing::debug!("{} paths in this batch", batch.len());

        let status = Command::new("nix")
            .args(["--extra-experimental-features", "nix-command"])
            .args(["copy", "--to", store_uri])
            .args(&batch)
            .env("PATH", &env_path)
            .status()
            .await?;

        if status.success() {
            tracing::debug!("Uploaded batch");
        } else {
            tracing::error!("Failed to upload batch: {:?}", status);
            return Err(Error::FailedToUpload);
        }
    }

    Ok(())
}
