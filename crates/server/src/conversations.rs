//! Structured native-client transport. The CLI owns the Agent loop and tool
//! execution; this module only supervises JSONL, persistence and UI decisions.
use crate::{ApiError, AppState, Result, db, providers, session_record};
use agentdock_domain::{InteractionMode, ProviderKind, Session, SessionId, SessionStatus};
use agentdock_persistence::MessageSubmission;
use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::Response,
    routing::{get, patch, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Notify, broadcast, mpsc},
};

#[derive(Clone, Default)]
pub struct ChatManager {
    sessions: Arc<Mutex<HashMap<SessionId, Arc<ChatRuntime>>>>,
    channels: Arc<Mutex<HashMap<SessionId, broadcast::Sender<Value>>>>,
}
pub struct ChatRuntime {
    commands: mpsc::Sender<Value>,
    stop: Notify,
    changed: Notify,
    done: Notify,
    running: AtomicBool,
    finished: AtomicBool,
    ready: AtomicBool,
    busy: AtomicBool,
    approvals: Mutex<HashMap<String, HashSet<String>>>,
    /// The bridge process; the native client runs beneath it.
    pid: Option<u32>,
}
#[cfg(unix)]
struct OwnedProcessGroup(Option<u32>);
#[cfg(unix)]
impl OwnedProcessGroup {
    fn signal(&self, signal: i32) {
        if let Some(pid) = self.0.filter(|pid| *pid > 1) {
            unsafe {
                libc::kill(-(pid as i32), signal);
            }
        }
    }
    fn finish(&mut self) {
        self.signal(libc::SIGKILL);
        self.0 = None;
    }
}
#[cfg(unix)]
impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        self.finish();
    }
}
impl ChatRuntime {
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    fn idle(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.busy.load(Ordering::Acquire)
            && self.approvals.lock().expect("approvals").is_empty()
    }
    async fn ready(&self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(15),async {
            loop {let changed=self.changed.notified();if !self.running(){return Err(ApiError::conflict("Native chat could not initialize; check the client installation and configuration"));}
                if self.ready.load(Ordering::Acquire){return Ok(());}changed.await;}
        }).await.map_err(|_|ApiError::conflict("Native chat initialization timed out"))?
    }
    async fn send(&self, value: Value) -> Result<()> {
        if !self.running() {
            return Err(ApiError::conflict(
                "Native chat is disconnected; reconnect before sending",
            ));
        }
        self.commands.try_send(value).map_err(|_|ApiError::conflict("Native chat command queue is unavailable; inspect the current state before retrying"))
    }
    async fn stop(&self) {
        let done = self.done.notified();
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        self.stop.notify_one();
        let _ = tokio::time::timeout(Duration::from_secs(6), done).await;
    }
}
impl ChatManager {
    /// Process ids of the conversations still running, for resource reporting.
    pub fn process_ids(&self) -> Vec<(SessionId, u32)> {
        self.sessions
            .lock()
            .expect("chat sessions")
            .iter()
            .filter(|(_, runtime)| runtime.running())
            .filter_map(|(id, runtime)| runtime.pid.map(|pid| (*id, pid)))
            .collect()
    }
    pub fn get(&self, id: SessionId) -> Option<Arc<ChatRuntime>> {
        self.sessions
            .lock()
            .expect("chat sessions")
            .get(&id)
            .cloned()
    }
    fn channel(&self, id: SessionId) -> broadcast::Sender<Value> {
        self.channels
            .lock()
            .expect("chat channels")
            .entry(id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
    fn publish(&self, id: SessionId, event: Value) {
        let _ = self.channel(id).send(event);
    }
    pub async fn stop(&self, id: SessionId) -> Result<()> {
        if let Some(runtime) = self.get(id) {
            runtime.stop().await;
            if !runtime.finished.load(Ordering::Acquire) {
                return Err(ApiError::conflict("Native chat is still shutting down"));
            }
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        let active: Vec<_> = self
            .sessions
            .lock()
            .expect("chat sessions")
            .values()
            .cloned()
            .collect();
        futures_util::future::join_all(active.iter().map(|r| r.stop())).await;
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/conversation", get(get_conversation))
        .route(
            "/api/sessions/{id}/conversation/open",
            post(open_conversation),
        )
        .route("/api/sessions/{id}/conversation/message", post(message))
        .route("/api/sessions/{id}/conversation/interrupt", post(interrupt))
        .route("/api/sessions/{id}/conversation/approval", post(approval))
        .route("/api/sessions/{id}/conversation/model", post(select_model))
        .route(
            "/api/sessions/{id}/conversation/permission",
            post(select_permission),
        )
        .route("/api/sessions/{id}/configuration", patch(configuration))
        .route("/api/sessions/{id}/chat/ws", get(socket))
}

async fn snapshot(state: &AppState, id: SessionId) -> Result<Value> {
    let session = session_record(state, id).await?;
    let stored = db(state, move |s| s.conversation(id)).await?;
    let running = if session.interaction_mode == InteractionMode::Structured {
        state.chats.get(id).is_some_and(|r| r.running())
    } else {
        state
            .runtime
            .get(&id.to_string())
            .is_some_and(|r| r.running())
    };
    Ok(
        json!({"type":"snapshot","mode":session.interaction_mode,"running":running,"events":stored.events,"truncated":stored.truncated}),
    )
}
async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Value>> {
    snapshot(&state, id).await.map(Json)
}

pub async fn persist(state: &AppState, id: SessionId, event: Value) -> Result<()> {
    let event = db(state, move |s| s.append_conversation_event(id, event)).await?;
    state.chats.publish(id, event);
    Ok(())
}

/// Caller holds operations. Initialization has no prompt/model request.
pub async fn start_locked(state: &AppState, session: &Session) -> Result<Arc<ChatRuntime>> {
    let id = session.id;
    if session.provider == ProviderKind::Terminal
        || session.interaction_mode != InteractionMode::Structured
    {
        return Err(ApiError::conflict(
            "This session uses the native terminal interface",
        ));
    }
    if state
        .runtime
        .get(&id.to_string())
        .is_some_and(|r| r.running())
    {
        return Err(ApiError::conflict(
            "End the original terminal session before enabling chat; it will not be taken over",
        ));
    }
    if let Some(existing) = state.chats.get(id) {
        if existing.running() {
            existing.ready().await?;
            // A start whose request was dropped mid-way -- the page reloaded
            // while the client was coming up -- leaves a running client
            // recorded as starting. The next start or open puts that right.
            if matches!(session.status, SessionStatus::Starting) {
                db(state, move |s| {
                    s.set_session_status(id, SessionStatus::Running)
                })
                .await?;
            }
            return Ok(existing);
        }
        // Do not let an old supervisor append its exit after a replacement has
        // begun emitting the next conversation's events.
        state.chats.stop(id).await?;
    }
    if state
        .chats
        .sessions
        .lock()
        .expect("chat sessions")
        .values()
        .filter(|r| r.running())
        .count()
        >= 32
    {
        return Err(ApiError::conflict(
            "At most 32 native chat processes can run at once",
        ));
    }
    let cwd = crate::checkouts::session_cwd(state, session).await?;
    let config_state = state.clone();
    let config_session = session.clone();
    let spec = tokio::task::spawn_blocking(move || {
        crate::providers::build(&config_state, &config_session, cwd)
    })
    .await
    .map_err(ApiError::internal)??;
    let runtime_bin = std::env::var_os("AGENTDOCK_JS_RUNTIME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("AGENTDOCK_NODE_BIN").filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "node".into());
    let mut command = tokio::process::Command::new(runtime_bin);
    command
        .arg(&state.chat_bridge)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in &spec.env_remove {
        command.env_remove(key);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| {
        ApiError::conflict("Native chat needs Node.js and the installed chat bridge")
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::conflict("Cannot open native chat input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::conflict("Cannot open native chat output"))?;
    let (commands, receiver) = mpsc::channel(8);
    let runtime = Arc::new(ChatRuntime {
        commands,
        stop: Notify::new(),
        changed: Notify::new(),
        done: Notify::new(),
        running: AtomicBool::new(true),
        finished: AtomicBool::new(false),
        ready: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        approvals: Mutex::new(HashMap::new()),
        pid: child.id(),
    });
    db(state, move |s| {
        s.set_session_status(id, SessionStatus::Starting)
    })
    .await?;
    state
        .chats
        .sessions
        .lock()
        .expect("chat sessions")
        .insert(id, runtime.clone());
    let mut init = json!({"type":"init","provider":session.provider,"program":spec.program,"args":spec.args,"cwd":spec.cwd});
    if let Some(native_id) = &session.provider_session_id {
        init["resume_id"] = json!(native_id);
    }
    tokio::spawn(supervise(
        state.clone(),
        id,
        runtime.clone(),
        child,
        stdin,
        stdout,
        receiver,
        init,
    ));
    if let Err(error) = runtime.ready().await {
        runtime.stop().await;
        db(state, move |s| {
            s.set_session_result(
                id,
                SessionStatus::Failed,
                Some("Native chat failed to initialize"),
            )
        })
        .await?;
        return Err(error);
    }
    // Every start takes the preferred permission mode. Failing to apply it is
    // not failing to start: the chip still shows the mode actually in force,
    // and the user can change it there.
    if let Some(mode) = crate::preferences::initial_permission(state, session).await? {
        let _ = runtime
            .send(json!({"type": "permission", "mode": mode}))
            .await;
    }
    db(state, move |s| {
        s.set_session_status(id, SessionStatus::Running)
    })
    .await?;
    Ok(runtime)
}

async fn open_conversation(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Session>> {
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    if session.provider == ProviderKind::Terminal {
        return Err(ApiError::bad(
            "Terminals do not support structured conversations",
        ));
    }
    if session.interaction_mode != InteractionMode::Structured {
        if state
            .runtime
            .get(&id.to_string())
            .is_some_and(|r| r.running())
            || !matches!(
                session.status,
                SessionStatus::Stopped | SessionStatus::Failed
            )
        {
            return Err(ApiError::conflict(
                "End the original terminal session before enabling chat; it will not be taken over",
            ));
        }
        if !db(&state, move |s| {
            s.set_interaction_mode(id, InteractionMode::Structured)
        })
        .await?
        {
            return Err(ApiError::conflict("Session state changed"));
        }
    }
    start_locked(&state, &session_record(&state, id).await?).await?;
    Ok(Json(session_record(&state, id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageInput {
    id: String,
    content: String,
    configuration_revision: Option<u64>,
}
type CommandAck = (StatusCode, Json<Value>);
fn accepted(id: Option<&str>, duplicate: bool) -> CommandAck {
    (
        StatusCode::ACCEPTED,
        Json(json!({"accepted":true,"id":id,"duplicate":duplicate})),
    )
}
async fn message(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<MessageInput>,
) -> Result<CommandAck> {
    if uuid::Uuid::parse_str(&input.id).is_err()
        || input.content.trim().is_empty()
        || input.content.len() > 32768
        || input.content.contains('\0')
    {
        return Err(ApiError::bad(
            "A message needs a UUID and 1–32768 bytes of text without NUL",
        ));
    }
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    let mid = input.id.clone();
    let receipt_content = input.content.clone();
    if let Some(matches) = db(&state, move |s| {
        s.chat_submission(id, &mid, &receipt_content)
    })
    .await?
    {
        return if matches {
            Ok(accepted(Some(&input.id), true))
        } else {
            Err(ApiError::conflict(
                "This message ID was already used for different text",
            ))
        };
    }
    if input
        .configuration_revision
        .is_some_and(|expected| expected != session.configuration_revision)
    {
        return Err(ApiError::conflict(
            "The session configuration changed. Refresh the selected endpoint before sending; this message was not dispatched.",
        ));
    }
    let runtime = start_locked(&state, &session).await?;
    if !runtime.idle() {
        return Err(ApiError::conflict(
            "Wait for the active turn and its approvals before sending another message",
        ));
    }
    let mid = input.id.clone();
    let content = input.content.clone();
    match db(&state, move |s| s.submit_chat_message(id, &mid, &content)).await? {
        MessageSubmission::Conflict => {
            return Err(ApiError::conflict(
                "This message ID was already used for different text",
            ));
        }
        MessageSubmission::Duplicate => return Ok(accepted(Some(&input.id), true)),
        MessageSubmission::New(event) => state.chats.publish(id, event),
    }
    runtime.busy.store(true, Ordering::Release);
    if let Err(error) = runtime
        .send(json!({"type":"message","id":input.id,"content":input.content}))
        .await
    {
        // try_send failed before the value entered the queue. Keep the durable
        // receipt to prevent replay, but do not leave an imaginary active turn.
        runtime.busy.store(false, Ordering::Release);
        persist(&state,id,json!({"type":"error","message":"Message dispatch failed. Inspect conversation state before resending; the message is never replayed automatically."})).await?;
        persist(
            &state,
            id,
            json!({"type":"turn","id":input.id,"status":"failed"}),
        )
        .await?;
        return Err(error);
    }
    Ok(accepted(Some(&input.id), false))
}
async fn interrupt(State(state): State<AppState>, Path(id): Path<SessionId>) -> Result<CommandAck> {
    let _guard = state.operations.lock().await;
    session_record(&state, id).await?;
    if let Some(runtime) = state.chats.get(id).filter(|r| r.running()) {
        runtime.send(json!({"type":"interrupt"})).await?;
    }
    Ok(accepted(None, false))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionInput {
    mode: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelInput {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}
/// Choose the model for a running session.
///
/// This is deliberately not an endpoint change: it neither bumps the
/// configuration revision nor restarts the bridge, because both native clients
/// switch models in place and the conversation's context must survive it. The
/// client validates the name and refuses one it cannot serve.
async fn select_model(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<ModelInput>,
) -> Result<CommandAck> {
    let _guard = state.operations.lock().await;
    session_record(&state, id).await?;
    let model = input
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    if let Some(model) = model.as_deref()
        && (model.len() > 128 || model.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Choose a model the client offers."));
    }
    if let Some(effort) = input.effort.as_deref()
        && !providers::EFFORT_LEVELS.contains(&effort)
    {
        return Err(ApiError::bad("Unsupported reasoning effort."));
    }
    if model.is_none() && input.effort.is_none() {
        return Err(ApiError::bad("Choose a model or a thinking depth."));
    }
    let runtime = state
        .chats
        .get(id)
        .filter(|r| r.running())
        .ok_or_else(|| ApiError::conflict("Start the session before choosing a model"))?;
    runtime
        .send(model_command(model.as_deref(), input.effort.as_deref()))
        .await?;
    Ok(accepted(None, false))
}
/// How tools get approved, for a session that is already running.
///
/// The names are AgentDock's and each bridge translates them, because the two
/// clients do not model this the same way: Claude Code switches a session-wide
/// mode, Codex decides per command. `danger` is the one that stops asking, and
/// it is named for what it costs rather than for what it saves.
///
/// Which modes a session actually offers comes from the client, in the same
/// settings event as its model list — Codex has no plan or accept-edits, and a
/// control that errors when used would be worse than one that is not there.
const PERMISSION_MODES: [&str; 4] = ["ask", "plan", "accept_edits", "danger"];

async fn select_permission(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<PermissionInput>,
) -> Result<CommandAck> {
    let _guard = state.operations.lock().await;
    session_record(&state, id).await?;
    if !PERMISSION_MODES.contains(&input.mode.as_str()) {
        return Err(ApiError::bad("Unsupported permission mode."));
    }
    let runtime = state
        .chats
        .get(id)
        .filter(|r| r.running())
        .ok_or_else(|| ApiError::conflict("Start the session before changing permissions"))?;
    runtime
        .send(json!({"type": "permission", "mode": input.mode}))
        .await?;
    Ok(accepted(None, false))
}

/// Absent means "leave the depth as it is". A `null` would reach the bridge as a
/// present-but-unusable value and be rejected, so the field is omitted entirely.
fn model_command(model: Option<&str>, effort: Option<&str>) -> Value {
    let mut command = json!({"type": "model"});
    if let Some(model) = model {
        command["model"] = Value::from(model);
    }
    if let Some(effort) = effort {
        command["effort"] = Value::from(effort);
    }
    command
}

#[cfg(test)]
mod event_whitelist_tests {
    use super::normalize_event;
    use serde_json::json;

    // The whitelist strips fields it does not name, and a stripped field is not
    // an error anywhere — the bridge emits it, the client never sees it, and
    // nothing says so. This already cost one silently dropped model list.
    #[test]
    fn subagent_progress_survives_normalisation() {
        let event = normalize_event(json!({
            "type": "tool", "id": "task-1", "name": "Task",
            "status": "running", "activity": "Scanning the bridge."
        }))
        .expect("a tool event carrying subagent progress must be accepted");
        assert_eq!(event["activity"], "Scanning the bridge.");
    }

    #[test]
    fn progress_is_optional_and_must_be_text() {
        assert!(
            normalize_event(json!({"type":"tool","id":"t","name":"Read","status":"completed"}))
                .is_some(),
            "a tool card without a subagent is still a valid event"
        );
        assert!(
            normalize_event(json!({
                "type":"tool","id":"t","name":"Read","status":"running","activity":{"nested":1}
            }))
            .is_none(),
            "progress must be text, not an arbitrary structure"
        );
    }

    #[test]
    fn unknown_fields_are_still_refused() {
        // Proving the guard is real: were it not, the test above would pass for
        // a field nobody added to the list.
        assert!(
            normalize_event(json!({
                "type":"tool","id":"t","name":"Read","status":"running","invented":"x"
            }))
            .is_none_or(|event| event.get("invented").is_none()),
            "a field outside the whitelist must not reach the client"
        );
    }
}

#[cfg(test)]
mod model_command_tests {
    use super::model_command;

    #[test]
    fn an_unset_field_is_left_out_rather_than_sent_as_null() {
        let command = model_command(Some("opus"), None);
        assert_eq!(command["model"], "opus");
        assert!(
            command.get("effort").is_none(),
            "a null effort is rejected by the bridge as a malformed level"
        );
        assert_eq!(model_command(Some("opus"), Some("high"))["effort"], "high");
        // A depth-only change keeps whichever model the session already runs.
        let depth_only = model_command(None, Some("high"));
        assert!(depth_only.get("model").is_none());
        assert_eq!(depth_only["effort"], "high");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalInput {
    request_id: String,
    decision: String,
    #[serde(default)]
    answers: HashMap<String, Vec<String>>,
}
async fn approval(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<ApprovalInput>,
) -> Result<CommandAck> {
    let _guard = state.operations.lock().await;
    session_record(&state, id).await?;
    let runtime = state
        .chats
        .get(id)
        .filter(|r| r.running())
        .ok_or_else(|| ApiError::conflict("The native request is no longer active"))?;
    if input.answers.len() > 16
        || input.answers.iter().any(|(k, v)| {
            k.len() > 200 || v.len() > 20 || v.iter().any(|s| s.len() > 8192 || s.contains('\0'))
        })
    {
        return Err(ApiError::bad("Approval answers exceed supported limits"));
    }
    if serde_json::to_vec(&input.answers)
        .map_err(ApiError::internal)?
        .len()
        > 128 * 1024
    {
        return Err(ApiError::bad(
            "Approval answers exceed the total size limit",
        ));
    }
    let allowed = runtime
        .approvals
        .lock()
        .expect("approvals")
        .get(&input.request_id)
        .is_some_and(|choices| choices.contains(&input.decision));
    if !allowed {
        return Err(ApiError::conflict(
            "The request expired or does not support this decision",
        ));
    }
    runtime.send(json!({"type":"approval","request_id":input.request_id,"decision":input.decision,"answers":input.answers})).await?;
    Ok(accepted(Some(&input.request_id), false))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationInput {
    endpoint_profile_id: Option<uuid::Uuid>,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    confirmed: bool,
}
async fn configuration(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<ConfigurationInput>,
) -> Result<Json<Session>> {
    if !input.confirmed {
        return Err(ApiError::bad(
            "Confirm a new native conversation before switching endpoints; displayed history is not forwarded",
        ));
    }
    if input
        .model
        .as_ref()
        .is_some_and(|m| m.len() > 200 || m.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Invalid model identifier"));
    }
    crate::providers::validate_effort(input.effort.as_deref())?;
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    if session.provider == ProviderKind::Terminal {
        return Err(ApiError::bad(
            "Terminal sessions do not use endpoint profiles",
        ));
    }
    let model = input.model.filter(|s| !s.trim().is_empty());
    let effort = input.effort.filter(|s| !s.trim().is_empty());
    if let Some(pid) = input.endpoint_profile_id {
        let profile = db(&state, move |s| s.get_endpoint_profile(pid))
            .await?
            .ok_or_else(|| ApiError::missing("Profile"))?;
        if profile.provider != session.provider {
            return Err(ApiError::bad("Choose an endpoint for the same CLI"));
        }
        crate::providers::validate_profile(&profile)?;
        if let Some(reference) = &profile.native_config {
            // A model and a depth are launch choices the client still resolves
            // against its own account, so an official account accepts them —
            // and for a model whose live switch the upstream refuses, choosing
            // it at launch is the only way through.
            crate::native_config::validate_reference(&state, &session.provider, reference)?;
        }
    }
    if state
        .runtime
        .get(&id.to_string())
        .is_some_and(|r| r.running())
    {
        return Err(ApiError::conflict(
            "End the native terminal session before changing its endpoint",
        ));
    }
    if let Some(runtime) = state.chats.get(id).filter(|r| r.running()) {
        if !runtime.idle() {
            return Err(ApiError::conflict(
                "Finish or interrupt the active turn and resolve approvals before switching",
            ));
        }
        state.chats.stop(id).await?;
        db(&state, move |s| {
            s.set_session_status(id, SessionStatus::Stopped)
        })
        .await?;
    }
    if !db(&state, move |s| {
        s.switch_session_configuration_with_effort(id, input.endpoint_profile_id, model, effort)
    })
    .await?
    {
        return Err(ApiError::conflict(
            "The session must be idle or stopped before changing its endpoint",
        ));
    }
    state.chats.publish(id, snapshot(&state, id).await?);
    Ok(Json(session_record(&state, id).await?))
}

/// Stop a structured session that is idle, before moving it; refuse one mid-turn
/// or waiting on an approval. A session not running needs nothing.
pub async fn stop_if_idle(state: &AppState, id: SessionId) -> Result<()> {
    if let Some(runtime) = state.chats.get(id).filter(|r| r.running()) {
        if !runtime.idle() {
            return Err(ApiError::conflict(
                "Finish or interrupt the active turn and resolve approvals before switching",
            ));
        }
        state.chats.stop(id).await?;
        db(state, move |s| {
            s.set_session_status(id, SessionStatus::Stopped)
        })
        .await?;
    }
    Ok(())
}
/// Tell connected views the conversation changed outside a turn.
pub async fn publish_snapshot(state: &AppState, id: SessionId) {
    if let Ok(value) = snapshot(state, id).await {
        state.chats.publish(id, value);
    }
}

async fn socket(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    session_record(&state, id).await?;
    Ok(upgrade
        .max_message_size(1024)
        .on_upgrade(move |socket| stream(state, id, socket)))
}
async fn stream(state: AppState, id: SessionId, socket: WebSocket) {
    let mut events = state.chats.channel(id).subscribe();
    let (mut sender, mut receiver) = socket.split();
    let mut last_seq = 0;
    let Ok(initial) = snapshot(&state, id).await else {
        return;
    };
    if let Some(seq) = initial["events"]
        .as_array()
        .and_then(|v| v.last())
        .and_then(|v| v["seq"].as_u64())
    {
        last_seq = seq;
    }
    if crate::send_frame(&mut sender, Message::Text(initial.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            event=events.recv()=> {
                let value=match event {Ok(value)=>value,Err(broadcast::error::RecvError::Lagged(_))=>match snapshot(&state,id).await{Ok(v)=>v,Err(_)=>break},Err(_)=>break};
                if let Some(seq)=value["seq"].as_u64(){if seq<=last_seq{continue;}last_seq=seq;}
                if value["type"]=="snapshot" && let Some(seq)=value["events"].as_array().and_then(|v|v.last()).and_then(|v|v["seq"].as_u64()){last_seq=seq;}
                if crate::send_frame(&mut sender,Message::Text(value.to_string().into())).await.is_err(){break;}
            }
            incoming=receiver.next()=>match incoming {Some(Ok(Message::Ping(bytes)))=>{if crate::send_frame(&mut sender,Message::Pong(bytes)).await.is_err(){break;}},Some(Ok(Message::Pong(_)))=>{},_=>break},
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    state: AppState,
    id: SessionId,
    runtime: Arc<ChatRuntime>,
    mut child: tokio::process::Child,
    mut stdin: tokio::process::ChildStdin,
    mut stdout: tokio::process::ChildStdout,
    mut commands: mpsc::Receiver<Value>,
    init: Value,
) {
    #[cfg(unix)]
    let mut group = OwnedProcessGroup(child.id());
    let mut buffer = Vec::new();
    let mut bytes = [0u8; 8192];
    let mut failure = None;
    let mut native_exit = false;
    if !matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            stdin.write_all(format!("{init}\n").as_bytes())
        )
        .await,
        Ok(Ok(()))
    ) {
        failure = Some("Native chat input closed during initialization");
    }
    while failure.is_none() && !native_exit {
        tokio::select! {
            _=runtime.stop.notified()=>break,
            command=commands.recv()=> {
                let Some(command)=command else{break;};
                if !matches!(tokio::time::timeout(Duration::from_secs(3),stdin.write_all(format!("{command}\n").as_bytes())).await,Ok(Ok(()))){failure=Some("Native chat input is unavailable");}
            }
            result=stdout.read(&mut bytes)=> {
                let count=match result {Ok(0)=>break,Ok(n)=>n,Err(_)=>{failure=Some("Native chat output closed unexpectedly");continue;}};
                buffer.extend_from_slice(&bytes[..count]);
                while let Some(end)=buffer.iter().position(|b|*b==b'\n') {
                    if end>256*1024 {failure=Some("Native chat event exceeded the size limit");break;}
                    let line:Vec<_>=buffer.drain(..=end).collect();
                    let event=match serde_json::from_slice::<Value>(&line).ok().and_then(normalize_event){Some(event)=>event,None=>{failure=Some("Native chat returned an unsupported event; update the installed bridge/client");break;}};
                    if event["type"]=="exit" {native_exit=true;break;}
                    if handle_event(&state,id,&runtime,event).await.is_err(){failure=Some("Could not persist native conversation; stopping to avoid losing output");break;}
                }
                if buffer.len()>256*1024 {failure=Some("Native chat event exceeded the size limit");}
            }
        }
    }
    if let Some(message) = failure {
        let _ = persist(&state, id, json!({"type":"error","message":message})).await;
    }
    let _ = tokio::time::timeout(
        Duration::from_millis(250),
        stdin.write_all(b"{\"type\":\"shutdown\"}\n"),
    )
    .await;
    // A dedicated, positively identified process group contains only this bridge
    // and its native CLI descendants. Never signal the host or unrelated Agents.
    #[cfg(unix)]
    {
        group.signal(libc::SIGTERM);
        // Keep the leader unreaped until its owned group has been cleaned up.
        // Waiting for only the Node PID can leave TERM-ignoring descendants.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        group.finish();
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    }
    runtime.running.store(false, Ordering::Release);
    runtime.ready.store(false, Ordering::Release);
    runtime.busy.store(false, Ordering::Release);
    runtime.changed.notify_waiters();
    let unresolved = runtime
        .approvals
        .lock()
        .expect("approvals")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for request_id in unresolved {
        let _ = persist(
            &state,
            id,
            json!({"type":"approval_resolved","id":request_id}),
        )
        .await;
    }
    runtime.approvals.lock().expect("approvals").clear();
    let _ = persist(&state, id, json!({"type":"exit"})).await;
    runtime.finished.store(true, Ordering::Release);
    runtime.done.notify_waiters();
    let _guard = state.operations.lock().await;
    if state
        .chats
        .get(id)
        .is_some_and(|current| Arc::ptr_eq(&current, &runtime))
    {
        let _ = db(&state, move |s| {
            if s.get_session(id)?.is_some_and(|record| {
                matches!(
                    record.status,
                    SessionStatus::Stopped | SessionStatus::Failed
                )
            }) {
                return Ok(true);
            }
            s.set_session_status(
                id,
                if failure.is_some() {
                    SessionStatus::Failed
                } else {
                    SessionStatus::Stopped
                },
            )
        })
        .await;
    }
}

fn normalize_event(mut value: Value) -> Option<Value> {
    let object = value.as_object_mut()?;
    let allowed: &[&str] = match object.get("type")?.as_str()? {
        "ready" => &["type", "native_session_id", "commands"],
        "settings" => &[
            "type",
            "model",
            "effort",
            "models",
            "permission_mode",
            "permission_modes",
        ],
        "cleared" => &["type"],
        "message" => &["type", "id", "role", "text", "delta"],
        "tool" => &["type", "id", "name", "status", "text", "activity"],
        "approval" => &["type", "id", "title", "text", "choices", "questions"],
        "approval_resolved" => &["type", "id"],
        "turn" => &["type", "id", "status"],
        "usage" => &[
            "type",
            "input_tokens",
            "output_tokens",
            "context_tokens",
            "context_window",
        ],
        "error" => &["type", "message"],
        "exit" => &["type"],
        _ => return None,
    };
    object.retain(|key, _| allowed.contains(&key.as_str()));
    let text = |key: &str| object.get(key).is_some_and(Value::is_string);
    let id = || {
        object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.len() <= 256)
    };
    let valid = match object.get("type").and_then(Value::as_str)? {
        "ready" => {
            object.get("native_session_id").is_none_or(Value::is_string)
                // Command names are forwarded from the client's own list, so a
                // malformed entry drops the list rather than reaching the UI.
                && object.get("commands").is_none_or(|value| {
                    value.as_array().is_some_and(|names| {
                        names.len() <= 400
                            && names.iter().all(|name| {
                                name.as_str().is_some_and(|name| {
                                    !name.is_empty()
                                        && name.len() <= 64
                                        && name.chars().all(|c| {
                                            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':')
                                        })
                                })
                            })
                    })
                })
        }
        // A model list is the client's own; a malformed entry drops the list
        // rather than reaching the picker as something half-known.
        // Lengths are counted in characters, because that is what the bridge
        // clips by. Counting bytes here rejected a whole model list over a
        // description that was within its limit but contained a multi-byte
        // character, and a rejected event looks to the user like a client that
        // offers no models at all.
        "settings" => {
            let chars = |value: &Value, limit: usize| {
                value.as_str().is_some_and(|v| v.chars().count() <= limit)
            };
            object.get("model").is_none_or(|value| {
                value
                    .as_str()
                    .is_some_and(|v| !v.is_empty() && v.chars().count() <= 128)
            }) && object.get("effort").is_none_or(|value| {
                value
                    .as_str()
                    .is_some_and(|v| providers::EFFORT_LEVELS.contains(&v))
            }) && object.get("models").is_none_or(|value| {
                value.as_array().is_some_and(|rows| {
                    rows.len() <= 64
                        && rows.iter().all(|row| {
                            row.get("id").is_some_and(|v| {
                                v.as_str().is_some_and(|v| !v.is_empty()) && chars(v, 128)
                            }) && row.get("name").is_some_and(|v| chars(v, 128))
                                && row.get("description").is_none_or(|v| chars(v, 256))
                                && row.get("isDefault").is_none_or(Value::is_boolean)
                                && row.get("efforts").is_none_or(|v| {
                                    v.as_array().is_some_and(|levels| {
                                        levels.len() <= 16
                                            && levels.iter().all(|level| {
                                                level.as_str().is_some_and(|level| {
                                                    providers::EFFORT_LEVELS.contains(&level)
                                                })
                                            })
                                    })
                                })
                        })
                })
            })
        }
        "message" => {
            id() && text("text")
                && matches!(
                    object.get("role").and_then(Value::as_str),
                    Some("user" | "assistant")
                )
                && object.get("delta").is_none_or(Value::is_boolean)
        }
        "tool" => {
            id() && text("name")
                && object.get("text").is_none_or(Value::is_string)
                // Subagent progress, kept apart from `text` so it cannot displace
                // the tool's own input and result.
                && object.get("activity").is_none_or(Value::is_string)
                && matches!(
                    object.get("status").and_then(Value::as_str),
                    Some("running" | "completed" | "failed")
                )
        }
        "approval" => {
            id() && text("title")
                && text("text")
                && object
                    .get("choices")
                    .and_then(Value::as_array)
                    .is_some_and(|choices| {
                        !choices.is_empty()
                            && choices.len() <= 3
                            && choices.iter().all(|v| {
                                matches!(v.as_str(), Some("accept" | "decline" | "cancel"))
                            })
                    })
                && object.get("questions").is_none_or(|value| {
                    value.as_array().is_some_and(|qs| {
                        qs.len() <= 16
                            && qs.iter().all(|q| {
                                q["id"].is_string()
                                    && q["question"].is_string()
                                    && q["options"].as_array().is_some_and(|options| {
                                        options.len() <= 32
                                            && options.iter().all(|o| o["label"].is_string())
                                    })
                            })
                    })
                })
        }
        "approval_resolved" => id(),
        "turn" => {
            object.get("id").is_none_or(Value::is_string)
                && matches!(
                    object.get("status").and_then(Value::as_str),
                    Some("running" | "completed" | "failed" | "interrupted")
                )
        }
        "usage" => {
            ["input_tokens", "output_tokens", "context_tokens", "context_window"]
                .iter()
                .all(|key| {
                    object
                        .get(*key)
                        .is_none_or(|v| v.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991))
                })
                // A zero window would make every share look like 100%, so it is
                // dropped rather than displayed.
                && object
                    .get("context_window")
                    .is_none_or(|v| v.as_u64().is_some_and(|n| n > 0))
        }
        "error" => text("message"),
        "exit" | "cleared" => true,
        _ => false,
    };
    if !valid {
        return None;
    }
    Some(value)
}
async fn handle_event(
    state: &AppState,
    id: SessionId,
    runtime: &ChatRuntime,
    event: Value,
) -> Result<()> {
    match event["type"].as_str() {
        Some("ready") => {
            if let Some(native_id) = event["native_session_id"].as_str() {
                if !crate::native_history::valid_id(native_id) {
                    return Err(ApiError::bad("Invalid native conversation identifier"));
                }
                let native_id = native_id.to_owned();
                db(state, move |s| s.set_native_conversation_id(id, &native_id)).await?;
            }
        }
        Some("turn") => match event["status"].as_str() {
            Some("running") => runtime.busy.store(true, Ordering::Release),
            Some("completed" | "failed" | "interrupted") => {
                runtime.busy.store(false, Ordering::Release)
            }
            _ => return Err(ApiError::bad("Invalid turn state")),
        },
        Some("approval") => {
            let request = event["id"]
                .as_str()
                .filter(|s| s.len() <= 200)
                .ok_or_else(|| ApiError::bad("Invalid approval identifier"))?;
            let choices = event["choices"]
                .as_array()
                .ok_or_else(|| ApiError::bad("Missing native approval choices"))?
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| matches!(*s, "accept" | "decline" | "cancel"))
                .map(str::to_owned)
                .collect();
            runtime
                .approvals
                .lock()
                .expect("approvals")
                .insert(request.into(), choices);
        }
        Some("approval_resolved") => {
            if let Some(request) = event["id"].as_str() {
                runtime.approvals.lock().expect("approvals").remove(request);
            }
        }
        Some("message") if event["role"] == "user" => return Ok(()), // The durable submission already contains this exact user message.
        _ => {}
    }
    let is_ready = event["type"] == "ready";
    persist(state, id, event).await?;
    if is_ready {
        runtime.ready.store(true, Ordering::Release);
        runtime.changed.notify_waiters();
    }
    Ok(())
}
