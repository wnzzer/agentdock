//! Tools an agent can use to see and configure AgentDock.
//!
//! An agent reaches these through `agentdock mcp`, a stdio MCP server that
//! only relays (mcp.rs). Everything that matters happens here: which tools
//! exist, who is calling, and what each caller may do. Keeping it on the
//! server means there is one place to check and one place to change.
//!
//! Callers identify themselves with a bearer token. A session AgentDock
//! launches gets its own token, issued at launch and good only while that
//! session's process is running, so a call is known to come from it. Any other caller that
//! passes the server's normal authentication -- `agentdock mcp` in a terminal
//! outside AgentDock -- is `External`. The token says who is asking; it is not
//! what protects anything sensitive. That is the confirmation a person gives
//! in the browser, which no token can skip.
use crate::{ApiError, AppState, db};
use agentdock_domain::{Session, SessionId};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Who is calling a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Session(SessionId),
    External,
}

#[derive(Clone, Default)]
pub struct AgentRegistry {
    tokens: Arc<Mutex<HashMap<String, SessionId>>>,
    /// Sessions the person has let start other sessions (agent_sessions.rs),
    /// until the server restarts.
    spawners: Arc<Mutex<std::collections::HashSet<SessionId>>>,
}

impl AgentRegistry {
    /// A fresh token for a session about to launch. Any earlier one for the
    /// same session is revoked: only the process being started holds a key.
    pub fn issue(&self, session: SessionId) -> String {
        let token = format!(
            "adk_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut tokens = self.tokens.lock().expect("agent tokens");
        tokens.retain(|_, owner| *owner != session);
        tokens.insert(token.clone(), session);
        token
    }
    pub fn may_spawn(&self, session: SessionId) -> bool {
        self.spawners.lock().expect("spawners").contains(&session)
    }
    pub fn allow_spawning(&self, session: SessionId) {
        self.spawners.lock().expect("spawners").insert(session);
    }
    fn session_for(&self, token: &str) -> Option<SessionId> {
        self.tokens
            .lock()
            .expect("agent tokens")
            .get(token)
            .copied()
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

/// Whether a session has a process running now, in either interface. A token
/// outlives nothing: once its process has ended it identifies no one.
pub(crate) fn live(state: &AppState, session: SessionId) -> bool {
    state.chats.get(session).is_some_and(|chat| chat.running())
        || state
            .runtime
            .get(&session.to_string())
            .is_some_and(|runtime| runtime.running())
}

fn session_caller(state: &AppState, headers: &HeaderMap) -> Option<SessionId> {
    bearer(headers)
        .and_then(|token| state.agents.session_for(token))
        .filter(|session| live(state, *session))
}

/// A request carrying a live session's agent token. The guard lets these
/// through to `/api/agent/` only, and nowhere else.
pub fn is_agent_request(state: &AppState, headers: &HeaderMap) -> bool {
    session_caller(state, headers).is_some()
}

fn caller(state: &AppState, headers: &HeaderMap) -> Caller {
    session_caller(state, headers).map_or(Caller::External, Caller::Session)
}

/// What a session's process needs to reach these tools: where the server is
/// and its own token. Set in its environment at every launch.
pub fn launch_environment(state: &AppState, session: SessionId) -> [(String, String); 3] {
    [
        ("AGENTDOCK_URL".into(), state.security.local_url()),
        ("AGENTDOCK_AGENT_TOKEN".into(), state.agents.issue(session)),
        ("AGENTDOCK_SESSION_ID".into(), session.to_string()),
    ]
}

/// Give a launch AgentDock's tools, unless the person turned them off.
///
/// Only through this launch's flags: the person's own client configuration
/// (`~/.claude`, `~/.codex`, or a host configuration a profile points at) is
/// never written. Claude reads a small private file named by `--mcp-config`,
/// and the tools are allowed up front, because what needs the person's word
/// is asked in AgentDock itself. Codex takes the same as `-c` overrides.
/// A terminal gets the environment only.
pub fn equip(
    state: &AppState,
    session: &Session,
    spec: &mut agentdock_runtime::SpawnSpec,
) -> Result<(), ApiError> {
    if !crate::preferences::load_blocking(state).agent_tools() {
        return Ok(());
    }
    let identity = launch_environment(state, session.id);
    for (key, value) in &identity {
        spec.env_remove.retain(|removed| removed != key);
        spec.env.insert(key.clone(), value.clone());
    }
    if session.provider == agentdock_domain::ProviderKind::Terminal {
        return Ok(());
    }
    let program = std::env::current_exe()
        .map_err(ApiError::internal)?
        .to_string_lossy()
        .into_owned();
    let environment: serde_json::Map<String, Value> = identity
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    match session.provider {
        agentdock_domain::ProviderKind::ClaudeCode => {
            let directory = crate::providers::private_dir(&state.state_dir.join("agent-mcp"))?;
            let path = directory.join(format!("{}.json", session.id));
            let config = json!({ "mcpServers": { "agentdock": {
                "type": "stdio", "command": program, "args": ["mcp"], "env": environment,
            } } });
            crate::security::write_owner_only(&path, &config.to_string())
                .map_err(ApiError::internal)?;
            // `=` form: both flags take several values and would swallow the
            // argument after them.
            spec.args
                .push(format!("--mcp-config={}", path.to_string_lossy()));
            spec.args.push("--allowedTools=mcp__agentdock".into());
        }
        agentdock_domain::ProviderKind::Codex => {
            let string = |value: &str| serde_json::to_string(value).expect("string");
            let table = identity
                .iter()
                .map(|(key, value)| format!("{key}={}", string(value)))
                .collect::<Vec<_>>()
                .join(",");
            // First, so they are global options before any subcommand.
            let overrides = [
                format!("mcp_servers.agentdock.command={}", string(&program)),
                r#"mcp_servers.agentdock.args=["mcp"]"#.to_owned(),
                format!("mcp_servers.agentdock.env={{{table}}}"),
                // A call waiting on the person's confirmation outlasts the default.
                "mcp_servers.agentdock.tool_timeout_sec=600".to_owned(),
            ];
            let mut args: Vec<String> = overrides
                .into_iter()
                .flat_map(|value| ["-c".to_owned(), value])
                .collect();
            args.append(&mut spec.args);
            spec.args = args;
        }
        agentdock_domain::ProviderKind::Terminal => {}
    }
    Ok(())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agent/tools", get(list_tools))
        .route("/api/agent/call", post(call_tool))
}

/// What the MCP server hands the model at connection time.
pub const INSTRUCTIONS: &str = "\
AgentDock is the workspace this agent may be running in: a local web app that runs Claude Code, Codex and terminals side by side, with workspaces, Git worktrees and endpoint profiles. These tools let you inspect it and, where the person allows, configure it.

- Start with agentdock_status to learn which session, workspace and endpoint you are.
- agentdock_list shows workspaces, sessions (titles and status only, never conversations), endpoint profiles and installed clients.
- agentdock_endpoint creates, changes, tests or imports endpoint profiles. To set up an API endpoint, call create with the provider, URL and model: the person confirms in the AgentDock window and types the key there. Then call test to check it works.
- Never ask the person to paste an API key into the conversation, and never put one in a tool argument. AgentDock collects keys in its own window.
- agentdock_preferences sets what new sessions start with; agentdock_workspace adds a directory as a workspace.
- agentdock_spawn starts another Claude Code or Codex session with a self-contained task, optionally on its own branch, for independent work worth doing in parallel. Follow it with agentdock_session (wait, result, message). You can only follow sessions you started, and a session you start cannot start more.
- agentdock_show opens a file at a line, your Git changes, or a session in the person's AgentDock window: use it to point at what you changed instead of describing where it is.
- agentdock_usage reports the quota your official account last reported.
- Changes that matter are confirmed by the person in the AgentDock window, and a call waits up to 5 minutes for that answer. If they decline, do not retry the same change unasked.";

pub(crate) fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

pub fn tools() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "agentdock_status",
            "Who you are in AgentDock: the calling session, its workspace and directory, its endpoint profile and model, and the AgentDock version.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "agentdock_list",
            "List AgentDock workspaces, sessions (metadata only), endpoint profiles, or installed agent clients.",
            json!({
                "type": "object",
                "properties": {
                    "what": { "type": "string", "enum": ["workspaces", "sessions", "endpoints", "clients"] },
                    "workspace_id": { "type": "string", "description": "Only with what=sessions: limit to one workspace." },
                    "include_archived": { "type": "boolean", "description": "Only with what=sessions. Default false." }
                },
                "required": ["what"],
                "additionalProperties": false
            }),
        ),
    ];
    tools.extend(crate::agent_config::tools());
    tools.extend(crate::agent_sessions::tools());
    tools
}

async fn list_tools() -> Json<Value> {
    Json(json!({ "instructions": INSTRUCTIONS, "tools": tools() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallInput {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// The MCP `tools/call` result shape. A tool that fails is still a successful
/// HTTP exchange: the model reads the error and can try something else.
fn result(outcome: Result<Value, ApiError>) -> Json<Value> {
    Json(match outcome {
        Ok(value) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
            "isError": false
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": error.message }],
            "isError": true
        }),
    })
}

async fn call_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CallInput>,
) -> Json<Value> {
    let caller = caller(&state, &headers);
    let arguments = if input.arguments.is_null() {
        json!({})
    } else {
        input.arguments
    };
    result(match input.name.as_str() {
        "agentdock_status" => status(&state, caller).await,
        "agentdock_list" => list(&state, caller, &arguments).await,
        "agentdock_endpoint" => crate::agent_config::endpoint(&state, caller, &arguments).await,
        "agentdock_preferences" => {
            crate::agent_config::preferences(&state, caller, &arguments).await
        }
        "agentdock_workspace" => crate::agent_config::workspace(&state, caller, &arguments).await,
        "agentdock_spawn" => crate::agent_sessions::spawn(&state, caller, &arguments).await,
        "agentdock_session" => crate::agent_sessions::session(&state, caller, &arguments).await,
        "agentdock_show" => crate::agent_sessions::show(&state, caller, &arguments).await,
        "agentdock_usage" => crate::agent_sessions::usage(&state, caller).await,
        other => Err(ApiError::bad(format!("Unknown AgentDock tool {other:?}"))),
    })
}

pub(crate) fn text<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn session_of(state: &AppState, caller: Caller) -> Result<Option<Session>, ApiError> {
    match caller {
        Caller::External => Ok(None),
        Caller::Session(id) => db(state, move |store| store.get_session(id)).await,
    }
}

/// How a profile's key is set, without saying what it is.
fn key_state(state_dir: &std::path::Path, reference: Option<&str>) -> &'static str {
    match reference.and_then(|value| value.strip_prefix("env:")) {
        None => "none",
        Some(name) if std::env::var(name).is_ok_and(|value| !value.is_empty()) => "environment",
        Some(name) if crate::secrets::resolve(state_dir, name).is_some() => "saved",
        Some(_) => "missing",
    }
}

pub(crate) fn endpoint_view(
    state_dir: &std::path::Path,
    profile: &agentdock_domain::EndpointProfile,
) -> Value {
    json!({
        "id": profile.id,
        "name": profile.name,
        "provider": profile.provider,
        "endpoint_url": profile.endpoint_url,
        "model": profile.model,
        "effort": profile.effort,
        "permission_mode": profile.permission_mode,
        "proxy_url": profile.proxy_url,
        "key": key_state(state_dir, profile.secret_ref.as_deref()),
        "host_configuration": profile.native_config.is_some(),
    })
}

fn session_view(session: &Session) -> Value {
    json!({
        "id": session.id,
        "title": session.title,
        "provider": session.provider,
        "status": session.status,
        "workspace_id": session.workspace_id,
        "temporary": session.ephemeral,
        "archived": session.archived_at.is_some(),
        "branch": session.checkout_branch,
    })
}

async fn status(state: &AppState, caller: Caller) -> Result<Value, ApiError> {
    let session = session_of(state, caller).await?;
    let you = match &session {
        None => {
            json!({ "kind": "external", "note": "Not a session AgentDock launched; session-specific tools are unavailable." })
        }
        Some(session) => {
            let workspace_id = session.workspace_id;
            let workspace = db(state, move |store| store.get_workspace(workspace_id)).await?;
            json!({
                "kind": "session",
                "session": session_view(session),
                "workspace": workspace.map(|w| json!({ "id": w.id, "name": w.name, "root_path": w.root_path })),
                "working_directory": session.checkout_path,
                "endpoint": session.endpoint_snapshot.as_ref().map(|profile| endpoint_view(&state.state_dir, profile)),
            })
        }
    };
    Ok(json!({ "agentdock": { "version": env!("CARGO_PKG_VERSION") }, "you": you }))
}

async fn list(state: &AppState, _caller: Caller, arguments: &Value) -> Result<Value, ApiError> {
    match text(arguments, "what") {
        Some("workspaces") => {
            let workspaces = db(state, |store| store.list_workspaces()).await?;
            Ok(json!(
                workspaces
                    .iter()
                    .map(|w| json!({ "id": w.id, "name": w.name, "root_path": w.root_path }))
                    .collect::<Vec<_>>()
            ))
        }
        Some("sessions") => {
            let workspace = text(arguments, "workspace_id")
                .map(|value| {
                    value
                        .parse::<agentdock_domain::WorkspaceId>()
                        .map_err(|_| ApiError::bad("workspace_id is not a workspace id"))
                })
                .transpose()?;
            let archived = arguments
                .get("include_archived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let sessions = db(state, move |store| store.list_sessions(workspace)).await?;
            Ok(json!(
                sessions
                    .iter()
                    .filter(|session| archived || session.archived_at.is_none())
                    .take(200)
                    .map(session_view)
                    .collect::<Vec<_>>()
            ))
        }
        Some("endpoints") => {
            let profiles = db(state, |store| store.list_endpoint_profiles()).await?;
            Ok(json!(
                profiles
                    .iter()
                    .map(|profile| endpoint_view(&state.state_dir, profile))
                    .collect::<Vec<_>>()
            ))
        }
        Some("clients") => {
            let mut clients = Vec::new();
            for provider in [
                agentdock_domain::ProviderKind::ClaudeCode,
                agentdock_domain::ProviderKind::Codex,
            ] {
                if let Some(view) = crate::clients::view(state, provider).await {
                    clients.push(json!({ "provider": view.provider, "installed": view.installed, "version": view.version }));
                }
            }
            Ok(json!(clients))
        }
        _ => Err(ApiError::bad(
            "what must be workspaces, sessions, endpoints or clients",
        )),
    }
}
