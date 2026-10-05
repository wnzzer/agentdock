//! Read-only access the person granted, from the browser, to a file or folder
//! outside the folders AgentDock may browse.
//!
//! A grant widens one thing only: the read-only preview of a file an agent
//! mentioned. It never opens browsing, editing or a workspace there. Grants
//! live in memory, so a restart ends them, and the routes sit outside
//! `/api/agent/`, where a session's own agent token cannot reach: an agent can
//! ask the person, never grant itself.
use crate::{ApiError, AppState, Result, workspace_io};
use axum::{
    Json, Router,
    extract::State,
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize)]
pub struct Grant {
    pub path: String,
    /// A folder and everything under it, or the one file.
    pub directory: bool,
    pub granted_at: String,
}

#[derive(Clone, Default)]
pub struct HostGrants(Arc<Mutex<Vec<Grant>>>);

impl HostGrants {
    /// The granted paths, as extra roots for a read.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.0
            .lock()
            .expect("host grants")
            .iter()
            .map(|grant| PathBuf::from(&grant.path))
            .collect()
    }
    fn list(&self) -> Vec<Grant> {
        self.0.lock().expect("host grants").clone()
    }
    fn add(&self, grant: Grant) {
        let mut grants = self.0.lock().expect("host grants");
        if !grants.iter().any(|existing| existing.path == grant.path) {
            grants.push(grant);
        }
        // A handful is all anyone needs; the oldest go first.
        let excess = grants.len().saturating_sub(32);
        grants.drain(..excess);
    }
    fn clear(&self) {
        self.0.lock().expect("host grants").clear();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantInput {
    path: String,
    #[serde(default)]
    directory: bool,
}

/// What a grant for `path` would cover, or why it is refused.
pub fn grant_target(path: &str, directory: bool) -> Result<(PathBuf, bool)> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(ApiError::bad("An absolute path is required"));
    }
    let target =
        dunce::canonicalize(requested).map_err(|_| ApiError::bad("That path does not exist"))?;
    let (target, directory) = if directory && target.is_file() {
        (
            target
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| ApiError::bad("That file has no folder"))?,
            true,
        )
    } else {
        (target.clone(), target.is_dir())
    };
    // The whole disk is not a temporary exception, and credentials and client
    // configuration stay out of reach however they are asked for.
    if target.parent().is_none() {
        return Err(ApiError::bad(
            "The root of the file system cannot be granted",
        ));
    }
    if workspace_io::is_protected_path(&target) {
        return Err(ApiError::bad(
            "That path holds credentials or configuration and cannot be granted",
        ));
    }
    Ok((target, directory))
}

async fn list(State(state): State<AppState>) -> Json<Vec<Grant>> {
    Json(state.host_grants.list())
}

async fn grant(
    State(state): State<AppState>,
    Json(input): Json<GrantInput>,
) -> Result<Json<Vec<Grant>>> {
    let (target, directory) = grant_target(&input.path, input.directory)?;
    state.host_grants.add(Grant {
        path: target.to_string_lossy().into_owned(),
        directory,
        granted_at: chrono::Utc::now().to_rfc3339(),
    });
    Ok(Json(state.host_grants.list()))
}

async fn revoke(State(state): State<AppState>) -> Json<Vec<Grant>> {
    state.host_grants.clear();
    Json(Vec::new())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/host/grants", get(list).post(grant))
        .route("/api/host/grants/all", delete(revoke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_covers_a_file_or_its_folder_and_never_the_root_or_credentials() {
        let base = std::env::temp_dir().join(format!("agentdock-grant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("logs")).unwrap();
        std::fs::write(base.join("logs/app.log"), "line").unwrap();
        let file = base.join("logs/app.log");
        let (target, directory) = grant_target(file.to_str().unwrap(), false).unwrap();
        assert_eq!(
            (target.clone(), directory),
            (dunce::canonicalize(&file).unwrap(), false)
        );
        let (folder, directory) = grant_target(file.to_str().unwrap(), true).unwrap();
        assert_eq!(folder, dunce::canonicalize(base.join("logs")).unwrap());
        assert!(directory);
        assert!(grant_target("relative/path", false).is_err());
        assert!(
            grant_target("/", true).is_err(),
            "the whole disk is not a grant"
        );
        std::fs::create_dir_all(base.join(".ssh")).unwrap();
        std::fs::write(base.join(".ssh/id_ed25519"), "secret").unwrap();
        assert!(grant_target(base.join(".ssh/id_ed25519").to_str().unwrap(), false).is_err());

        let grants = HostGrants::default();
        grants.add(Grant {
            path: folder.to_string_lossy().into_owned(),
            directory: true,
            granted_at: String::new(),
        });
        grants.add(Grant {
            path: folder.to_string_lossy().into_owned(),
            directory: true,
            granted_at: String::new(),
        });
        assert_eq!(grants.roots(), vec![folder]);
        grants.clear();
        assert!(grants.roots().is_empty());
        std::fs::remove_dir_all(base).ok();
    }
}
