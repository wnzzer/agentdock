//! Supported metadata APIs only. Import links history; it never adopts an OS process.
use crate::{ApiError, AppState};
use agentdock_domain::{ProviderKind, Session, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::{env, ffi::OsString, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct NativeSource {
    pub id: String,
    pub provider: ProviderKind,
    pub label: String,
    pub config_dir: PathBuf,
    /// Preserve unset vs explicit directory env: native keychain identity can differ.
    pub config_env: Option<OsString>,
}
#[derive(Serialize)]
pub struct SourceView {
    pub id: String,
    pub provider: ProviderKind,
    pub label: String,
    pub path: String,
    pub available: bool,
    pub config_env: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryItem {
    pub id: String,
    pub provider: ProviderKind,
    pub title: String,
    pub cwd: String,
    pub updated_at: String,
    pub imported_session_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
pub struct HistoryList {
    pub items: Vec<HistoryItem>,
    pub truncated: bool,
}
pub fn sources() -> Vec<NativeSource> {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let mut sources = Vec::new();
    for (id, provider, label, override_key, native_key, subdir) in [
        (
            "codex-default",
            ProviderKind::Codex,
            "Codex",
            "AGENTDOCK_CODEX_HISTORY_DIR",
            "CODEX_HOME",
            ".codex",
        ),
        (
            "claude-default",
            ProviderKind::ClaudeCode,
            "Claude Code",
            "AGENTDOCK_CLAUDE_HISTORY_DIR",
            "CLAUDE_CONFIG_DIR",
            ".claude",
        ),
    ] {
        let explicit = env::var_os(override_key)
            .filter(|value| !value.is_empty())
            .or_else(|| env::var_os(native_key))
            .filter(|value| !value.is_empty());
        let path = explicit
            .clone()
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(subdir)));
        if let Some(config_dir) = path {
            sources.push(NativeSource {
                id: id.into(),
                provider,
                label: label.into(),
                config_dir,
                config_env: explicit,
            });
        }
    }
    sources
}
pub fn source_views(sources: &[NativeSource]) -> Vec<SourceView> {
    sources
        .iter()
        .map(|s| SourceView {
            id: s.id.clone(),
            provider: s.provider.clone(),
            label: s.label.clone(),
            path: s.config_dir.to_string_lossy().into_owned(),
            available: s.config_dir.is_dir(),
            config_env: s
                .config_env
                .as_ref()
                .map(|value| value.to_string_lossy().into_owned()),
        })
        .collect()
}
pub fn selected_source<'a>(state: &'a AppState, id: &str) -> Result<&'a NativeSource, ApiError> {
    state
        .native_sources
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::missing("Native history source"))
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Node.js is the bundled bridge's default runtime. Retain the earlier Node
/// override for existing deployments, with the runtime-neutral setting taking
/// precedence. An empty setting is treated as absent, not as an executable.
pub(crate) fn js_runtime(primary: Option<OsString>, legacy: Option<OsString>) -> OsString {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| legacy.filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "node".into())
}

pub async fn list(
    state: &AppState,
    workspace: WorkspaceId,
    source_id: &str,
) -> Result<HistoryList, ApiError> {
    let cwd = crate::root(state, workspace).await?;
    let source = selected_source(state, source_id)?;
    if !source.config_dir.is_dir() {
        return Ok(HistoryList {
            items: Vec::new(),
            truncated: false,
        });
    }
    let config_dir = crate::paths::canonicalize_async(&source.config_dir)
        .await
        .map_err(|_| ApiError::bad("Native history directory is unavailable"))?;
    let job = serde_json::json!({"provider":source.provider,"config_dir":config_dir,"cwd":cwd});
    let mut command = tokio::process::Command::new(js_runtime(
        env::var_os("AGENTDOCK_JS_RUNTIME"),
        env::var_os("AGENTDOCK_NODE_BIN"),
    ));
    command
        .arg(&state.native_bridge)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in env::vars()
        .filter(|(key, _)| key.starts_with("AGENTDOCK_SECRET_") || key == "AGENTDOCK_TOKEN")
    {
        command.env_remove(key);
    }
    let mut child = command.spawn().map_err(|_| {
        ApiError::bad(
            "Native history needs Node.js and the installed native bridge; AGENTDOCK_JS_RUNTIME can override the runtime",
        )
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::bad("Cannot open history bridge input"))?;
    stdin
        .write_all(job.to_string().as_bytes())
        .await
        .map_err(ApiError::internal)?;
    drop(stdin);
    let output = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::bad("Cannot open history bridge output"))?;
    let result = tokio::time::timeout(Duration::from_secs(16), async {
        let mut bytes = Vec::new();
        output
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(ApiError::internal)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(ApiError::bad("History list exceeds limit"));
        }
        let status = child.wait().await.map_err(ApiError::internal)?;
        let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::bad("Native history bridge could not read this client version")
        })?;
        if !status.success() || raw.get("error").is_some() {
            return Err(ApiError::bad(
                raw.get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Native history query failed"),
            ));
        }
        serde_json::from_value::<HistoryList>(raw).map_err(ApiError::internal)
    })
    .await;
    let mut result = match result {
        Ok(result) => result,
        Err(_) => Err(ApiError::bad("Native history query timed out")),
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    if let Ok(ref mut listing) = result {
        listing.items.retain(|item| {
            valid_id(&item.id)
                && item.provider == source.provider
                && dunce::canonicalize(&item.cwd).is_ok_and(|p| p == cwd)
        });
        let managed = state
            .store
            .list_sessions(Some(workspace))
            .map_err(ApiError::internal)?;
        for item in &mut listing.items {
            item.title = item.title.chars().take(240).collect();
            item.imported_session_id = managed
                .iter()
                .find(|s| {
                    s.native_source_id.as_deref() == Some(source_id)
                        && s.provider_session_id.as_deref() == Some(&item.id)
                })
                .map(|s| s.id.to_string());
        }
    }
    result
}
pub async fn import(
    state: &AppState,
    workspace: WorkspaceId,
    source_id: String,
    native_id: String,
) -> Result<Session, ApiError> {
    import_with_environment(state, workspace, source_id, native_id, None).await
}

pub async fn import_with_environment(
    state: &AppState,
    workspace: WorkspaceId,
    source_id: String,
    native_id: String,
    environment: Option<agentdock_domain::EnvironmentOverrides>,
) -> Result<Session, ApiError> {
    if let Some(values) = &environment {
        crate::environment::validate(values)?;
    }
    if !valid_id(&native_id) {
        return Err(ApiError::bad("Invalid native session ID"));
    }
    let listing = list(state, workspace, &source_id).await?;
    let item = listing
        .items
        .into_iter()
        .find(|s| s.id == native_id)
        .ok_or_else(|| ApiError::missing("Native session in this workspace"))?;
    let source = selected_source(state, &source_id)?;
    let config = crate::paths::canonicalize_async(&source.config_dir)
        .await
        .map_err(|_| ApiError::bad("Native history source unavailable"))?;
    let source_id = source.id.clone();
    let dir = config.to_string_lossy().into_owned();
    crate::db(state, move |s| {
        s.import_native_session_with_environment(
            workspace,
            item.provider,
            &item.title,
            &source_id,
            &native_id,
            &dir,
            environment,
        )
    })
    .await?.ok_or_else(|| ApiError::conflict("Existing native session has different environment overrides; stop it and edit its environment explicitly"))
}
pub fn resume_spec(
    state: &AppState,
    session: &Session,
    cwd: PathBuf,
) -> Result<agentdock_runtime::SpawnSpec, ApiError> {
    let source = selected_source(
        state,
        session
            .native_source_id
            .as_deref()
            .ok_or_else(|| ApiError::bad("Missing native history source"))?,
    )?;
    if source.provider != session.provider {
        return Err(ApiError::bad("Native history provider mismatch"));
    }
    let directory = dunce::canonicalize(&source.config_dir)
        .map_err(|_| ApiError::bad("Original client configuration directory is unavailable"))?;
    if session.native_config_dir.as_deref() != directory.to_str() {
        return Err(ApiError::bad(
            "The original history source changed; load the session again from the intended source",
        ));
    }
    let native_id = session
        .provider_session_id
        .as_ref()
        .filter(|id| valid_id(id))
        .ok_or_else(|| ApiError::bad("Missing native session ID"))?;
    let (program, args, key) = match session.provider {
        ProviderKind::Codex => (
            crate::clients::program(&state.state_dir, &session.provider),
            vec!["resume".into(), native_id.clone()],
            "CODEX_HOME",
        ),
        ProviderKind::ClaudeCode => (
            crate::clients::program(&state.state_dir, &session.provider),
            vec!["--resume".into(), native_id.clone()],
            "CLAUDE_CONFIG_DIR",
        ),
        _ => return Err(ApiError::bad("Terminal has no imported history")),
    };
    let config_env = source
        .config_env
        .as_ref()
        .map(|value| {
            value
                .to_str()
                .ok_or_else(|| ApiError::bad("Native configuration path must be valid UTF-8"))
        })
        .transpose()?;
    let (environment, remove) = crate::native_config::launch_environment(key, config_env);
    Ok(agentdock_runtime::SpawnSpec {
        program,
        args,
        cwd,
        env: environment,
        env_remove: remove,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_runtime_defaults_to_node_and_respects_both_overrides() {
        assert_eq!(js_runtime(None, None), OsString::from("node"));
        assert_eq!(
            js_runtime(Some("/opt/runtime/bun".into()), Some("node".into())),
            OsString::from("/opt/runtime/bun")
        );
        assert_eq!(
            js_runtime(None, Some("/legacy/node".into())),
            OsString::from("/legacy/node")
        );
        assert_eq!(
            js_runtime(Some(OsString::new()), Some("legacy-node".into())),
            OsString::from("legacy-node")
        );
        assert_eq!(
            js_runtime(Some(OsString::new()), Some(OsString::new())),
            OsString::from("node")
        );
    }

    #[test]
    fn native_identifiers_cannot_be_paths_flags_or_unbounded_input() {
        for id in ["thread_01", "019aa-bb", "A"] {
            assert!(valid_id(id), "{id:?}");
        }
        for id in [
            "",
            "../thread",
            "/thread",
            "--all",
            "a b",
            "a\n",
            "a\0",
            "编号",
        ] {
            assert!(!valid_id(id), "{id:?}");
        }
        assert!(!valid_id(&"a".repeat(129)));
        assert!(valid_id(&"a".repeat(128)));
    }
}
