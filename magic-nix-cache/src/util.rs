//! Utilities.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use attic::nix_store::NixStore;

use crate::error::Result;

/// Returns the list of store paths that are currently present.
pub async fn get_store_paths(store: &NixStore) -> Result<HashSet<PathBuf>> {
    // FIXME: use the Nix API.
    let store_dir = store.store_dir();
    let mut listing = tokio::fs::read_dir(store_dir).await?;
    let mut paths = HashSet::new();
    while let Some(entry) = listing.next_entry().await? {
        let file_name = entry.file_name();
        let file_name = Path::new(&file_name);

        if let Some(extension) = file_name.extension() {
            match extension.to_str() {
                None | Some("drv") | Some("chroot") => {
                    tracing::debug!(
                        "skipping file with weird or uninteresting extension {extension:?}"
                    );
                    continue;
                }
                _ => {}
            }
        }

        if let Some(s) = file_name.to_str() {
            // Special paths (so far only `.links`)
            if s == ".links" {
                continue;
            }
        }

        paths.insert(store_dir.join(file_name));
    }
    Ok(paths)
}
