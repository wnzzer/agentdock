//! What a new session starts with, per provider: which endpoint profile, how
//! deeply it thinks, and which permission mode it runs in.
//!
//! Stored by the server rather than the browser, so a phone and a laptop open
//! sessions the same way. Each value is a default, not a rule: the new-session
//! dialog still shows and can change the first two, and the permission chip in
//! a running session changes the third.
use crate::{ApiError, AppState, Result, db, providers};
use agentdock_domain::{ProviderKind, Session};
use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderPreference {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_profile_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub claude_code: ProviderPreference,
    #[serde(default)]
    pub codex: ProviderPreference,
}

impl Preferences {
    pub fn for_provider(&self, provider: &ProviderKind) -> Option<&ProviderPreference> {
        match provider {
            ProviderKind::ClaudeCode => Some(&self.claude_code),
            ProviderKind::Codex => Some(&self.codex),
            _ => None,
        }
    }
}

/// The modes each client offers; Codex has no plan or accept-edits mode.
fn permission_modes(provider: &ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::ClaudeCode => &["ask", "plan", "accept_edits", "danger"],
        ProviderKind::Codex => &["ask", "danger"],
        _ => &[],
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/preferences", get(read).put(write))
}

pub async fn load(state: &AppState) -> Result<Preferences> {
    let stored = db(state, |s| s.preferences()).await?;
    // A document that no longer parses -- written by a later version, say --
    // is treated as no preferences rather than failing every new session.
    Ok(serde_json::from_value(stored).unwrap_or_default())
}

async fn read(State(state): State<AppState>) -> Result<Json<Preferences>> {
    Ok(Json(load(&state).await?))
}

async fn write(
    State(state): State<AppState>,
    Json(input): Json<Preferences>,
) -> Result<Json<Preferences>> {
    for (provider, preference) in [
        (ProviderKind::ClaudeCode, &input.claude_code),
        (ProviderKind::Codex, &input.codex),
    ] {
        providers::validate_effort(preference.effort.as_deref())?;
        if let Some(mode) = preference.permission.as_deref()
            && !permission_modes(&provider).contains(&mode)
        {
            return Err(ApiError::bad("Unsupported permission mode."));
        }
        if let Some(id) = preference.endpoint_profile_id {
            let profile = db(&state, move |s| s.get_endpoint_profile(id))
                .await?
                .ok_or_else(|| ApiError::missing("Profile"))?;
            if profile.provider != provider {
                return Err(ApiError::bad("Profile and preference providers must match"));
            }
        }
    }
    let value = serde_json::to_value(&input).map_err(ApiError::internal)?;
    db(&state, move |s| s.set_preferences(&value)).await?;
    Ok(Json(input))
}

/// The permission mode a session's client should switch to when it starts.
/// The clients do not carry a mode across a restart -- a resumed Claude Code
/// comes back asking -- so the preference applies at every start, not only
/// the first; without one, the client's own default stands.
pub async fn initial_permission(state: &AppState, session: &Session) -> Result<Option<String>> {
    let preferences = load(state).await?;
    Ok(preferences
        .for_provider(&session.provider)
        .and_then(|p| p.permission.clone())
        .filter(|mode| permission_modes(&session.provider).contains(&mode.as_str())))
}
