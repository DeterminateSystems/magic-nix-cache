//! Utilities.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use attic::nix_store::NixStore;

use crate::error::Result;

/// Returns the list of store paths that are currently present.
pub async fn get_store_paths(store: &NixStore) -> Result<HashSet<PathBuf>> {
    // FIXME(cole-h): update the nix bindings to get the dbdir of the localstore?
    let db =
        rusqlite::Connection::open("file:/nix/var/nix/db/db.sqlite?immutable=1").expect("FIXME");

    let mut stmt = db.prepare("SELECT path FROM ValidPaths").expect("FIXME");
    let paths = stmt
        .query_map([], |row| -> std::result::Result<PathBuf, rusqlite::Error> {
            Ok(PathBuf::from(row.get::<_, String>(0)?))
        })
        .expect("FIXME")
        .into_iter()
        .map(|r| r.expect("FIXME"))
        .collect::<HashSet<_>>();

    Ok(paths)
}
