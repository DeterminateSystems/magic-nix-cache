//! Utilities.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::{fs, process::Command};

use crate::error::{Error, Result};

/// Returns the list of store paths that are currently present.
pub async fn get_store_paths() -> Result<HashSet<PathBuf>> {
    let store_dir = Path::new("/nix/store");
    let mut listing = fs::read_dir(store_dir).await?;
    let mut paths = HashSet::new();
    while let Some(entry) = listing.next_entry().await? {
        let file_name = entry.file_name();
        let file_name = Path::new(&file_name);

        if let Some(extension) = file_name.extension() {
            match extension.to_str() {
                None | Some("drv") | Some("lock") => {
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
        }

        paths.insert(store_dir.join(file_name));
    }
    Ok(paths)
}

/// Uploads a list of store paths to the cache.
pub async fn upload_paths(mut paths: Vec<PathBuf>) -> Result<()> {
    // When the daemon started Nix may not have been installed
    let env_path = Command::new("sh")
        .args(&["-lc", "echo $PATH"])
        .output()
        .await?
        .stdout;
    let env_path = String::from_utf8(env_path)
        .expect("PATH contains invalid UTF-8");

    while !paths.is_empty() {
        let mut batch = Vec::new();
        let mut total_len = 0;

        while !paths.is_empty() && total_len < 1024 * 1024 {
            let p = paths.pop().unwrap();
            total_len += p.as_os_str().len() + 1;
            batch.push(p);
        }

        tracing::debug!("{} paths in this batch", batch.len());

        let status = Command::new("nix")
            .args(&["--extra-experimental-features", "nix-command"])
            // FIXME: Port and compression settings
            .args(&["copy", "--to", "http://127.0.0.1:3000"])
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
