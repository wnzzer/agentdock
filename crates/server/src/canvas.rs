//! Shared canvas persistence and explicit pane ownership validation.
//! Workspace ownership is metadata only here: this module never reads project
//! files, client configuration, or credentials to validate a saved layout.

use crate::{ApiError, AppState, Result};
use agentdock_domain::{ProviderKind, WorkspaceId};
use agentdock_persistence::Store;
use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    path::{Component, Path},
};
use uuid::Uuid;

const MAX_LAYOUT_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveLayout {
    layout: Value,
    expected_revision: u64,
}

pub async fn get_layout(State(state): State<AppState>) -> Result<Json<Value>> {
    let saved = crate::db(&state, |store| store.get_shared_canvas_layout()).await?;
    let layout = saved
        .layout
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()
        .map_err(ApiError::internal)?;
    // Corrupt state must surface as an error, not silently turn into an empty
    // canvas or a fallback project layout that could then overwrite user data.
    if let Some(layout) = &layout {
        validate_shape(layout)
            .map_err(|_| ApiError::internal("Stored shared canvas layout is invalid"))?;
    }
    Ok(Json(json!({"layout":layout,"revision":saved.revision})))
}

pub async fn save_layout(
    State(state): State<AppState>,
    Json(input): Json<SaveLayout>,
) -> Result<Json<Value>> {
    validate_shape(&input.layout)?;
    let store = state.store.clone();
    let revision = tokio::task::spawn_blocking(move || {
        validate_ownership(&store, &input.layout["root"], &mut HashSet::new())?;
        store.save_shared_canvas_layout(&input.layout.to_string(), input.expected_revision)
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::conflict("Shared canvas changed in another client; reload the latest layout before saving"))
    }).await.map_err(ApiError::internal)??;
    Ok(Json(json!({"revision":revision})))
}

fn validate_shape(layout: &Value) -> Result<()> {
    if !crate::valid_layout(layout) || layout.to_string().len() > MAX_LAYOUT_BYTES {
        return Err(ApiError::bad("Invalid or oversized shared canvas layout"));
    }
    if let Some(collapsed) = layout.get("collapsed")
        && !collapsed.as_array().is_some_and(Vec::is_empty)
    {
        return Err(ApiError::bad(
            "Restore legacy collapsed panes to the canonical layout tree before saving the shared canvas",
        ));
    }
    Ok(())
}

fn validate_ownership(
    store: &Store,
    node: &Value,
    workspaces: &mut HashSet<WorkspaceId>,
) -> Result<()> {
    match node["type"].as_str() {
        Some("split") => {
            validate_ownership(store, &node["first"], workspaces)?;
            validate_ownership(store, &node["second"], workspaces)
        }
        Some("stack") => {
            for pane in node["panes"].as_array().expect("validated stack") {
                validate_ownership(store, pane, workspaces)?;
            }
            Ok(())
        }
        Some("pane") => validate_pane(store, node, workspaces),
        _ => Err(ApiError::bad("Invalid canvas node")),
    }
}

fn validate_pane(store: &Store, pane: &Value, workspaces: &mut HashSet<WorkspaceId>) -> Result<()> {
    let kind = pane["kind"].as_str().expect("validated pane kind");
    let empty = Map::new();
    let metadata = match pane.get("metadata") {
        None => &empty,
        Some(value) => value
            .as_object()
            .ok_or_else(|| ApiError::bad("Pane metadata must be an object"))?,
    };
    let workspace_id = metadata
        .get("workspace_id")
        .map(|value| parse_id(value, "workspace_id"))
        .transpose()?;
    if let Some(workspace_id) = workspace_id
        && !workspaces.contains(&workspace_id)
    {
        if store
            .get_workspace(workspace_id)
            .map_err(ApiError::internal)?
            .is_none()
        {
            return Err(ApiError::missing("Canvas workspace"));
        }
        workspaces.insert(workspace_id);
    }

    let path = metadata.get("path");
    let session_id = metadata.get("session_id");
    if (matches!(kind, "git_diff" | "file_preview") || path.is_some() || session_id.is_some())
        && workspace_id.is_none()
    {
        return Err(ApiError::bad(
            "Bound session/file and Git panes require an explicit workspace_id",
        ));
    }
    if let Some(path) = path {
        validate_relative_path(path)?;
    }
    if let Some(session_id) = session_id {
        if !matches!(kind, "agent_chat" | "terminal") {
            return Err(ApiError::bad(
                "Only agent_chat or terminal panes may bind a session_id",
            ));
        }
        let session_id = parse_id(session_id, "session_id")?;
        let session = store
            .get_session(session_id)
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::missing("Canvas session"))?;
        if Some(session.workspace_id) != workspace_id {
            return Err(ApiError::bad(
                "Canvas session belongs to a different workspace",
            ));
        }
        if (kind == "terminal") != (session.provider == ProviderKind::Terminal) {
            return Err(ApiError::bad(
                "Canvas pane kind does not match its registered session",
            ));
        }
        if let Some(provider) = metadata.get("provider") {
            let provider = serde_json::from_value::<ProviderKind>(provider.clone())
                .map_err(|_| ApiError::bad("Invalid session pane provider"))?;
            if session.provider != provider {
                return Err(ApiError::bad(
                    "Canvas session provider does not match its registered provider",
                ));
            }
        }
    }
    Ok(())
}

fn parse_id(value: &Value, field: &str) -> Result<Uuid> {
    value
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ApiError::bad(format!("Pane {field} must be a UUID")))
}

fn validate_relative_path(value: &Value) -> Result<()> {
    let value = value
        .as_str()
        .ok_or_else(|| ApiError::bad("Pane path must be a relative path string"))?;
    if value.as_bytes().contains(&0)
        || Path::new(value).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ApiError::bad(
            "Pane path must stay relative to its workspace; absolute paths and traversal are not allowed",
        ));
    }
    // Do not interpret backslashes as separators on Unix: they are legal
    // filename characters. Actual file access retains workspace_io's checks.
    Ok(())
}
