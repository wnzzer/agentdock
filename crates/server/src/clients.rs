//! Native client discovery and optional npm installation.
//!
//! AgentDock drives the user's installed Claude Code and Codex clients, so the
//! first question for a new host is simply whether they are there. When one is
//! missing AgentDock can install it from npm into its own state directory. That
//! copy is a fallback, never an override: a client already on `PATH` keeps
//! winning, so installing here cannot silently change which binary a session
//! has been using.
use crate::{ApiError, AppState, Result};
use agentdock_domain::ProviderKind;
use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

/// Where a resolved program came from. Surfaced so the page can say which copy
/// a session will actually run instead of only whether one exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramSource {
    /// An explicit `AGENTDOCK_*_BIN` override on the server.
    Override,
    /// Found on the server's `PATH`.
    Path,
    /// Installed by AgentDock into its state directory.
    Managed,
    /// Nothing found; the command name is kept so errors stay recognisable.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientView {
    pub provider: ProviderKind,
    pub command: String,
    pub npm_package: String,
    pub program: String,
    pub source: ProgramSource,
    pub installed: bool,
    pub version: Option<String>,
    pub managed_path: Option<String>,
    /// npm has to exist on the server before an install can be offered at all.
    pub install_available: bool,
    /// A managed copy exists but `PATH` wins, so installing changed nothing for
    /// sessions. Saying so avoids an install that looks like a no-op.
    pub shadowed: bool,
}

pub fn npm_package(provider: &ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::ClaudeCode => Some("@anthropic-ai/claude-code"),
        ProviderKind::Codex => Some("@openai/codex"),
        ProviderKind::Terminal => None,
    }
}
pub fn command(provider: &ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::ClaudeCode => Some("claude"),
        ProviderKind::Codex => Some("codex"),
        ProviderKind::Terminal => None,
    }
}
fn override_key(provider: &ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::ClaudeCode => Some("AGENTDOCK_CLAUDE_BIN"),
        ProviderKind::Codex => Some("AGENTDOCK_CODEX_BIN"),
        ProviderKind::Terminal => None,
    }
}

pub fn managed_root(state_dir: &Path) -> PathBuf {
    state_dir.join("clients")
}
fn managed_program(state_dir: &Path, command: &str) -> Option<PathBuf> {
    let path = managed_root(state_dir)
        .join("node_modules/.bin")
        .join(command);
    executable(&path).then_some(path)
}

fn executable(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            true
        }
        _ => false,
    }
}

/// Resolve a command against the server's own `PATH`, the same way spawning it
/// by bare name would.
pub fn on_path(command: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(command))
        .find(|candidate| executable(candidate))
}

/// The program a session will actually launch, and where it came from.
pub fn resolve(state_dir: &Path, provider: &ProviderKind) -> (String, ProgramSource) {
    let Some(command) = command(provider) else {
        return (String::new(), ProgramSource::Missing);
    };
    if let Some(value) = override_key(provider)
        .and_then(env::var_os)
        .filter(|value| !value.is_empty())
    {
        return (
            value.to_string_lossy().into_owned(),
            ProgramSource::Override,
        );
    }
    // PATH before the managed copy: installing from this page must never
    // repoint sessions away from the client the host already had.
    if let Some(path) = on_path(command) {
        return (path.to_string_lossy().into_owned(), ProgramSource::Path);
    }
    if let Some(path) = managed_program(state_dir, command) {
        return (path.to_string_lossy().into_owned(), ProgramSource::Managed);
    }
    (command.into(), ProgramSource::Missing)
}

/// Program name for spawning. Callers that only need something to exec keep
/// using this; the bare command name stays the fallback so a missing client
/// still fails with its own recognisable error.
pub fn program(state_dir: &Path, provider: &ProviderKind) -> String {
    resolve(state_dir, provider).0
}

async fn version(program: &str) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(program)
            .arg("--version")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.chars().take(120).collect())
}

pub async fn view(state: &AppState, provider: ProviderKind) -> Option<ClientView> {
    let command = command(&provider)?;
    let package = npm_package(&provider)?;
    let (program, source) = resolve(&state.state_dir, &provider);
    let managed = managed_program(&state.state_dir, command);
    let version = if source == ProgramSource::Missing {
        None
    } else {
        version(&program).await
    };
    Some(ClientView {
        provider,
        command: command.into(),
        npm_package: package.into(),
        source,
        installed: source != ProgramSource::Missing && version.is_some(),
        version,
        shadowed: managed.is_some() && source != ProgramSource::Managed,
        managed_path: managed.map(|path| path.to_string_lossy().into_owned()),
        install_available: on_path("npm").is_some(),
        program,
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/clients", get(list))
        .route("/api/clients/{provider}/install", post(install))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<ClientView>>> {
    let mut views = Vec::new();
    for provider in [ProviderKind::ClaudeCode, ProviderKind::Codex] {
        if let Some(view) = view(&state, provider).await {
            views.push(view);
        }
    }
    Ok(Json(views))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    confirmed: bool,
}

async fn install(
    State(state): State<AppState>,
    RoutePath(provider): RoutePath<String>,
    Json(input): Json<Install>,
) -> Result<Json<ClientView>> {
    if !input.confirmed {
        return Err(ApiError::bad(
            "Installing a native client downloads and runs package code; explicit confirmation is required",
        ));
    }
    let provider = match provider.as_str() {
        "claude_code" => ProviderKind::ClaudeCode,
        "codex" => ProviderKind::Codex,
        _ => return Err(ApiError::bad("Only Claude Code and Codex can be installed")),
    };
    let package =
        npm_package(&provider).ok_or_else(|| ApiError::bad("No package for this client"))?;
    let npm = on_path("npm").ok_or_else(|| {
        ApiError::bad("Installing a native client needs npm on the server's PATH")
    })?;
    // One install at a time: concurrent npm runs share a cache and a prefix.
    let _guard = state.operations.lock().await;
    let root = managed_root(&state.state_dir);
    fs::create_dir_all(&root).map_err(ApiError::internal)?;
    let output = tokio::time::timeout(
        Duration::from_secs(600),
        tokio::process::Command::new(npm)
            .args([
                "install",
                "--no-fund",
                "--no-audit",
                "--prefix",
                &root.to_string_lossy(),
                package,
            ])
            .current_dir(&root)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| ApiError::bad("The client install did not finish in time"))?
    .map_err(|_| ApiError::bad("The client install could not be started"))?;
    if !output.status.success() {
        // npm's own diagnostic is the useful part; keep the tail, not a guess.
        let text = String::from_utf8_lossy(&output.stderr);
        let detail: String = text
            .trim()
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(ApiError::bad(if detail.is_empty() {
            "The client install failed. Check npm on the server.".to_owned()
        } else {
            format!("The client install failed: {detail}")
        }));
    }
    view(&state, provider)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::internal("Installed client could not be inspected"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_copies_never_override_a_client_the_host_already_has() {
        let root = env::temp_dir().join(format!("agentdock-clients-{}", uuid::Uuid::new_v4()));
        let bin = managed_root(&root).join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        let managed = bin.join("codex");
        fs::write(&managed, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Nothing on PATH: the managed copy is the fallback that gets used.
        let restore = env::var_os("PATH");
        // SAFETY: single-threaded test of process-wide resolution.
        unsafe { env::set_var("PATH", "") };
        assert_eq!(
            resolve(&root, &ProviderKind::Codex),
            (
                managed.to_string_lossy().into_owned(),
                ProgramSource::Managed
            )
        );
        // A host copy on PATH wins, so an install here cannot repoint sessions.
        let host = root.join("host-bin");
        fs::create_dir_all(&host).unwrap();
        let host_codex = host.join("codex");
        fs::write(&host_codex, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&host_codex, fs::Permissions::from_mode(0o755)).unwrap();
        }
        unsafe { env::set_var("PATH", &host) };
        assert_eq!(
            resolve(&root, &ProviderKind::Codex),
            (
                host_codex.to_string_lossy().into_owned(),
                ProgramSource::Path
            )
        );
        // An explicit server override still beats both.
        unsafe { env::set_var("AGENTDOCK_CODEX_BIN", "/explicit/codex") };
        assert_eq!(
            resolve(&root, &ProviderKind::Codex),
            ("/explicit/codex".to_owned(), ProgramSource::Override)
        );
        unsafe { env::remove_var("AGENTDOCK_CODEX_BIN") };
        // A missing client keeps its bare command name so its own error shows.
        unsafe { env::set_var("PATH", "") };
        assert_eq!(
            resolve(&root, &ProviderKind::ClaudeCode),
            ("claude".to_owned(), ProgramSource::Missing)
        );
        match restore {
            // SAFETY: restores the process default for other tests.
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        assert!(npm_package(&ProviderKind::Terminal).is_none());
        let _ = fs::remove_dir_all(&root);
    }
}
