//! The agent tool for the canvas: which tabs are open, and closing them.
//!
//! The canvas is shared: every window shows the one layout the server keeps.
//! So a change is made there, on the stored layout, with the same checks and
//! revision compare-and-swap a window's own save goes through, and every open
//! window is told to load it. That works with no window open, and several
//! windows cannot race to apply the same change.
//!
//! Closing a tab closes a view, never a session -- except a temporary one,
//! which is discarded with its last tab, as it is when the person closes it.
//! A temporary session still working is closed only once the person agrees.
use crate::{
    ApiError, AppState,
    activity::{Answer, Undo},
    agent::{Caller, live, text},
    agent_config::who,
    db,
};
use agentdock_domain::SessionId;
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn tools() -> Vec<Value> {
    vec![crate::agent::tool(
        "agentdock_canvas",
        "The tabs open on the AgentDock canvas: list them, or close some or all. Closing a tab only closes the view; the session keeps running, except a temporary session, which is discarded (a working one only after the person agrees).",
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "close"] },
                "pane_ids": { "type": "array", "items": { "type": "string" }, "description": "Tabs to close, by the pane_id list returns." },
                "all": { "type": "boolean", "description": "Close every tab." }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    )]
}

/// Every tab in the layout tree, in reading order.
fn panes<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    match node["type"].as_str() {
        Some("pane") => out.push(node),
        Some("stack") => out.extend(node["panes"].as_array().into_iter().flatten()),
        Some("split") => {
            panes(&node["first"], out);
            panes(&node["second"], out);
        }
        _ => {}
    }
}

fn ids(node: &Value, out: &mut HashSet<String>) {
    if let Some(id) = node["id"].as_str() {
        out.insert(id.to_owned());
    }
    for child in ["first", "second"] {
        if node.get(child).is_some() {
            ids(&node[child], out);
        }
    }
    for pane in node["panes"].as_array().into_iter().flatten() {
        ids(pane, out);
    }
}

/// The layout without one tab, as the web app's own closePane leaves it: a
/// stack keeps its other tabs (and a sensible active one), an emptied stack
/// goes, and a split with one side gone becomes the other side.
fn remove(node: &Value, id: &str) -> Option<Value> {
    match node["type"].as_str() {
        Some("pane") => (node["id"] != id).then(|| node.clone()),
        Some("stack") => {
            let tabs = node["panes"].as_array().cloned().unwrap_or_default();
            let Some(index) = tabs.iter().position(|pane| pane["id"] == id) else {
                return Some(node.clone());
            };
            let kept: Vec<Value> = tabs.into_iter().filter(|pane| pane["id"] != id).collect();
            if kept.is_empty() {
                return None;
            }
            let mut next = node.clone();
            if !kept.iter().any(|pane| pane["id"] == node["activePaneId"]) {
                next["activePaneId"] = kept[index.min(kept.len() - 1)]["id"].clone();
            }
            next["panes"] = Value::Array(kept);
            Some(next)
        }
        Some("split") => {
            let first = remove(&node["first"], id);
            let second = remove(&node["second"], id);
            match (first, second) {
                (None, other) | (other, None) => other,
                (Some(first), Some(second)) => {
                    let mut next = node.clone();
                    next["first"] = first;
                    next["second"] = second;
                    Some(next)
                }
            }
        }
        _ => Some(node.clone()),
    }
}

fn close(root: &Value, targets: &[String]) -> Value {
    let mut occupied = HashSet::new();
    ids(root, &mut occupied);
    let mut current = Some(root.clone());
    for id in targets {
        current = current.and_then(|node| remove(&node, id));
    }
    current.unwrap_or_else(|| {
        let mut id = "empty-workspace".to_owned();
        let mut suffix = 1;
        while occupied.contains(&id) {
            suffix += 1;
            id = format!("empty-workspace-{suffix}");
        }
        json!({ "type": "stack", "kind": "stack", "id": id, "panes": [] })
    })
}

async fn stored(state: &AppState) -> Result<(Option<Value>, u64), ApiError> {
    let saved = db(state, |s| s.get_shared_canvas_layout()).await?;
    let layout = saved
        .layout
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()
        .map_err(ApiError::internal)?;
    Ok((layout, saved.revision))
}

fn session_of(pane: &Value) -> Option<SessionId> {
    pane["metadata"]["session_id"]
        .as_str()
        .and_then(|id| id.parse().ok())
}

pub async fn canvas(
    state: &AppState,
    caller: Caller,
    arguments: &Value,
) -> Result<Value, ApiError> {
    let (layout, revision) = stored(state).await?;
    let Some(layout) = layout else {
        return Ok(json!({ "tabs": [], "note": "No canvas has been saved yet." }));
    };
    let mut open = Vec::new();
    panes(&layout["root"], &mut open);
    match text(arguments, "action") {
        Some("list") => {
            let mut tabs = Vec::new();
            for pane in &open {
                let session = match session_of(pane) {
                    Some(id) => db(state, move |s| s.get_session(id)).await?,
                    None => None,
                };
                tabs.push(json!({
                    "pane_id": pane["id"],
                    "kind": pane["kind"],
                    "title": pane["title"],
                    "session_id": pane["metadata"]["session_id"],
                    "path": pane["metadata"]["path"],
                    "workspace_id": pane["metadata"]["workspace_id"],
                    "temporary": session.as_ref().is_some_and(|s| s.ephemeral),
                    "running": session.as_ref().is_some_and(|s| live(state, s.id)),
                }));
            }
            Ok(json!({ "tabs": tabs }))
        }
        Some("close") => {
            let all = arguments
                .get("all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let named: Vec<String> = arguments["pane_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect();
            let present: HashSet<&str> =
                open.iter().filter_map(|pane| pane["id"].as_str()).collect();
            let targets: Vec<String> = if all {
                present.iter().map(|id| (*id).to_owned()).collect()
            } else {
                if named.is_empty() {
                    return Err(ApiError::bad("Give pane_ids, or all: true"));
                }
                let unknown: Vec<&String> = named
                    .iter()
                    .filter(|id| !present.contains(id.as_str()))
                    .collect();
                if !unknown.is_empty() {
                    return Err(ApiError::bad(format!(
                        "Not open on the canvas: {unknown:?}. Call list for the open tabs."
                    )));
                }
                named
            };
            if targets.is_empty() {
                return Ok(json!({ "closed": 0 }));
            }
            // A temporary session goes with its last tab; one still working
            // is the person's call.
            let mut temporary = Vec::new();
            let mut working = Vec::new();
            for pane in open
                .iter()
                .filter(|pane| targets.iter().any(|id| pane["id"] == id.as_str()))
            {
                if let Some(id) = session_of(pane)
                    && let Some(session) = db(state, move |s| s.get_session(id)).await?
                    && session.ephemeral
                {
                    if live(state, id) {
                        working.push((id, session.title.clone()));
                    }
                    temporary.push(id);
                }
            }
            let who = who(state, caller).await;
            if !working.is_empty() {
                let rows = working
                    .iter()
                    .map(|(_, title)| ("Temporary session", title.clone()))
                    .collect();
                let answer = state
                    .activity
                    .ask(
                        who.clone(),
                        "Close {count} tabs",
                        json!({ "count": targets.len() }),
                        rows,
                        vec!["Temporary sessions still working are stopped and discarded with their tab."],
                        None,
                    )
                    .await?;
                if !matches!(answer, Answer::Approved { .. }) {
                    return Err(ApiError::conflict(
                        "The person declined. No tab was closed.",
                    ));
                }
            }
            // Retry on a race with a window's own save: apply to the latest.
            let mut attempt = (layout.clone(), revision);
            let saved = loop {
                let mut next = attempt.0.clone();
                next["root"] = close(&attempt.0["root"], &targets);
                next.as_object_mut()
                    .map(|object| object.remove("collapsed"));
                let store = state.store.clone();
                let expected = attempt.1;
                let candidate = next.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::canvas::save_checked(&store, &candidate, expected)
                })
                .await
                .map_err(ApiError::internal)??;
                if let Some(revision) = result {
                    break revision;
                }
                let (latest, revision) = stored(state).await?;
                if revision <= attempt.1 {
                    return Err(ApiError::conflict(
                        "The canvas changed while closing tabs; try again",
                    ));
                }
                attempt = (latest.unwrap_or_else(|| layout.clone()), revision);
            };
            for id in &temporary {
                let _ = crate::discard_session(
                    axum::extract::State(state.clone()),
                    axum::extract::Path(*id),
                )
                .await;
            }
            state.activity.canvas_changed(saved);
            state.activity.notify(
                who,
                "Closed {count} tabs",
                json!({ "count": targets.len() }),
                temporary
                    .is_empty()
                    .then(|| Undo::Canvas(Box::new(attempt.0))),
            );
            Ok(json!({ "closed": targets.len(), "discarded_temporary_sessions": temporary.len() }))
        }
        _ => Err(ApiError::bad("action must be list or close")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> Value {
        json!({ "type": "pane", "id": id, "kind": "agent_chat", "title": id })
    }

    #[test]
    fn closing_tabs_folds_the_layout_like_the_web_app() {
        let root = json!({ "type": "split", "id": "root", "direction": "horizontal", "ratio": 0.5,
            "first": { "type": "stack", "id": "left", "activePaneId": "b", "panes": [pane("a"), pane("b"), pane("c")] },
            "second": pane("d") });
        let one = close(&root, &["b".into()]);
        assert_eq!(one["first"]["panes"].as_array().unwrap().len(), 2);
        assert_eq!(one["first"]["activePaneId"], "c");
        let folded = close(&root, &["d".into()]);
        assert_eq!(
            folded["id"], "left",
            "a split with one side gone becomes the other side"
        );
        let empty = close(&root, &["a".into(), "b".into(), "c".into(), "d".into()]);
        assert_eq!(
            empty,
            json!({ "type": "stack", "kind": "stack", "id": "empty-workspace", "panes": [] })
        );
    }
}
