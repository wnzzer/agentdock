//! How much context each model takes: the window the user set for a model,
//! and the published one (price_list.rs) as a fallback for showing it.
//!
//! Only a window the user set reaches a client. A published window can be one
//! the account has not been given (a 1M beta, say), so pushing it would make
//! the client overrun what it actually has. Claude Code gets the user's window
//! through `CLAUDE_CODE_MAX_CONTEXT_TOKENS`, set as an environment value that
//! follows the main model (environment.rs), Codex through
//! `model_context_window` (providers.rs).
use crate::{ApiError, AppState, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Smaller than any model worth running; larger than any that exists.
const FEWEST: u64 = 1_000;
const MOST: u64 = 100_000_000;
const MOST_MODELS: usize = 500;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelLimits {
    /// By model ID, as the client sends it, lowercased.
    #[serde(default)]
    pub models: BTreeMap<String, Limit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub context_window: u64,
}

fn path(state_dir: &Path) -> PathBuf {
    state_dir.join("model-limits.json")
}

fn key(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

pub fn load(state_dir: &Path) -> ModelLimits {
    std::fs::read(path(state_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The window the user set for `model`, if any.
pub fn context_override(state_dir: &Path, model: &str) -> Option<u64> {
    load(state_dir)
        .models
        .get(&key(model))
        .map(|limit| limit.context_window)
}

fn validate(limits: ModelLimits) -> Result<ModelLimits> {
    if limits.models.len() > MOST_MODELS {
        return Err(ApiError::bad("Too many model limits"));
    }
    let mut clean = ModelLimits::default();
    for (model, limit) in limits.models {
        let model = key(&model);
        if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
            return Err(ApiError::bad("Invalid model ID"));
        }
        if !(FEWEST..=MOST).contains(&limit.context_window) {
            return Err(ApiError::bad(
                "A context window is between 1,000 and 100,000,000 tokens",
            ));
        }
        clean.models.insert(model, limit);
    }
    Ok(clean)
}

fn save(state_dir: &Path, limits: &ModelLimits) -> Result<()> {
    static WRITE: Mutex<()> = Mutex::new(());
    let _guard = WRITE.lock().expect("model limits lock");
    let target = path(state_dir);
    let temporary = target.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(limits).map_err(ApiError::internal)?;
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, &target))
        .map_err(ApiError::internal)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Context {
    pub model: String,
    /// The window to show: the user's, else the published one.
    pub context_window: Option<u64>,
    /// `yours` or `published`; absent when neither is known.
    pub source: Option<&'static str>,
    /// The published window, shown beside the user's as a reference.
    pub published: Option<u64>,
    pub max_output: Option<u64>,
}

pub fn context(state_dir: &Path, model: &str) -> Context {
    let listed = crate::price_list::list().lookup(model);
    let published = listed.and_then(|price| price.context_window);
    let yours = context_override(state_dir, model);
    Context {
        model: model.to_owned(),
        context_window: yours.or(published),
        source: yours.map(|_| "yours").or(published.map(|_| "published")),
        published,
        max_output: listed.and_then(|price| price.max_output),
    }
}

async fn get_limits(State(state): State<AppState>) -> Json<ModelLimits> {
    Json(load(&state.state_dir))
}

/// Replace every limit the user set.
async fn put_limits(
    State(state): State<AppState>,
    Json(input): Json<ModelLimits>,
) -> Result<Json<ModelLimits>> {
    let clean = validate(input)?;
    let state_dir = state.state_dir.clone();
    let saved = clean.clone();
    tokio::task::spawn_blocking(move || save(&state_dir, &saved))
        .await
        .map_err(ApiError::internal)??;
    Ok(Json(clean))
}

#[derive(Deserialize)]
struct ContextQuery {
    model: String,
}

async fn get_context(
    State(state): State<AppState>,
    Query(query): Query<ContextQuery>,
) -> Result<Json<Context>> {
    let model = query.model.trim();
    if model.is_empty() || model.len() > 200 {
        return Err(ApiError::bad("Name a model"));
    }
    Ok(Json(context(&state.state_dir, model)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/models/limits", get(get_limits).put(put_limits))
        .route("/api/models/context", get(get_context))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("agentdock-limits-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn limits(entries: &[(&str, u64)]) -> ModelLimits {
        ModelLimits {
            models: entries
                .iter()
                .map(|(model, window)| {
                    (
                        (*model).to_owned(),
                        Limit {
                            context_window: *window,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn a_window_the_user_set_wins_and_the_published_one_is_the_fallback() {
        let dir = directory();
        let gateway = "claude-fable-5-dd-lacol-b9-5.3newq";
        assert_eq!(context(&dir, gateway).context_window, None, "unknown");
        let published = context(&dir, "gpt-5.5");
        assert_eq!(published.source, Some("published"));
        assert!(published.context_window.is_some());

        save(
            &dir,
            &validate(limits(&[(gateway, 32_768), ("GPT-5.5", 272_000)])).unwrap(),
        )
        .unwrap();
        let yours = context(&dir, gateway);
        assert_eq!(
            (yours.context_window, yours.source),
            (Some(32_768), Some("yours"))
        );
        let both = context(&dir, "gpt-5.5");
        assert_eq!(both.context_window, Some(272_000), "case does not matter");
        assert_eq!(
            both.published, published.context_window,
            "the published one stays visible"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_limit_must_be_a_sane_window_for_a_real_model_id() {
        assert!(validate(limits(&[("m", 999)])).is_err());
        assert!(validate(limits(&[("m", 200_000_000)])).is_err());
        assert!(validate(limits(&[(" ", 32_000)])).is_err());
        assert!(validate(limits(&[("m\u{0}", 32_000)])).is_err());
        assert!(
            validate(limits(&[("M", 32_000)]))
                .unwrap()
                .models
                .contains_key("m")
        );
    }
}
