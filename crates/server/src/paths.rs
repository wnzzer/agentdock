//! Canonical paths that other programs can use.
//!
//! On Windows `std::fs::canonicalize` returns verbatim `\\?\C:\…` paths. They
//! are canonical, but they reach git, Node and the native clients as working
//! directories and the browser as workspace roots, and several of those do not
//! understand the prefix. `dunce` drops it whenever the ordinary form means the
//! same thing, and is exactly `std::fs::canonicalize` everywhere else.
use std::{
    io,
    path::{Path, PathBuf},
};

pub async fn canonicalize_async(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    tokio::task::spawn_blocking(move || dunce::canonicalize(path))
        .await
        .map_err(io::Error::other)?
}
