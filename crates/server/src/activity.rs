//! What agents are doing to AgentDock, as the person sees it.
//!
//! A tool call that changes something the person should decide on waits here
//! for an answer from the browser: a card with what would change, and a key
//! field when the change needs a key, so the key goes from the person straight
//! to AgentDock and never through the agent. Everything else an agent changes
//! is reported as a notice, with an undo where one makes sense.
//!
//! These routes are deliberately not under `/api/agent/`, which is the only
//! place an agent's token opens: an agent cannot see, answer or undo its own
//! requests. Only a signed-in browser can.
use crate::{ApiError, AppState, db};
use agentdock_domain::EndpointProfile;
use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

/// How long a request waits for the person before the agent is told no one
/// answered. Long enough to walk back to the machine; short enough that an
/// agent is not stuck for an afternoon.
const ANSWER_WITHIN: Duration = Duration::from_secs(300);
const NOTICES_KEPT: usize = 50;

#[derive(Debug)]
pub enum Answer {
    Approved { secret: Option<String> },
    Declined,
}

/// What undoing a change does. Kept in memory for the last few notices.
#[derive(Debug, Clone)]
pub enum Undo {
    /// Remove a profile an agent created, and the key saved for it.
    DeleteProfile { id: Uuid, secret: Option<String> },
    /// Put back a profile as it was before an agent changed or deleted it.
    RestoreProfile(Box<EndpointProfile>),
    /// Put back the preferences document as it was.
    Preferences(Value),
    /// Put back the canvas layout as it was before tabs were closed.
    Canvas(Box<Value>),
}

struct Pending {
    view: Value,
    reply: oneshot::Sender<Answer>,
}

struct Inner {
    events: broadcast::Sender<Value>,
    pending: Mutex<HashMap<Uuid, Pending>>,
    undo: Mutex<VecDeque<(Uuid, Undo)>>,
}

#[derive(Clone)]
pub struct Activity {
    inner: Arc<Inner>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                events: broadcast::channel(64).0,
                pending: Mutex::new(HashMap::new()),
                undo: Mutex::new(VecDeque::new()),
            }),
        }
    }
}

/// A key the person is asked for along with a confirmation.
pub struct KeyField {
    pub label: String,
    /// Whether approving without a key is allowed.
    pub optional: bool,
}

/// Removes a request the moment its caller stops waiting -- answered, timed
/// out, or the agent's connection dropped -- so no card outlives its question.
struct Waiting<'a> {
    activity: &'a Activity,
    id: Uuid,
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self
            .activity
            .inner
            .pending
            .lock()
            .expect("pending")
            .remove(&self.id)
            .is_some()
        {
            let _ = self
                .activity
                .inner
                .events
                .send(json!({ "type": "resolved", "id": self.id }));
        }
    }
}

impl Activity {
    /// What a browser would see, for tests that play the person.
    #[cfg(test)]
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.events.subscribe()
    }

    fn watched(&self) -> bool {
        self.inner.events.receiver_count() > 0
    }

    /// Ask the person, and wait for the answer. `title` is an English
    /// template the browser translates, with `{name}`-style placeholders
    /// filled from `values`; `rows` are field/value pairs whose labels are
    /// translated and whose values are shown as they are; `notes` are
    /// sentences worth a second look, translated as they are.
    pub async fn ask(
        &self,
        who: Value,
        title: &str,
        values: Value,
        rows: Vec<(&str, String)>,
        notes: Vec<&str>,
        key: Option<KeyField>,
    ) -> Result<Answer, ApiError> {
        if !self.watched() {
            return Err(ApiError::conflict(
                "No AgentDock window is open to confirm this. Ask the person to open AgentDock, then try again.",
            ));
        }
        let id = Uuid::new_v4();
        let (reply, answer) = oneshot::channel();
        let view = json!({
            "type": "request",
            "id": id,
            "who": who,
            "title": title,
            "values": values,
            "rows": rows.into_iter().map(|(label, value)| json!({ "label": label, "value": value })).collect::<Vec<_>>(),
            "notes": notes,
            "key": key.map(|key| json!({ "label": key.label, "optional": key.optional })),
        });
        self.inner.pending.lock().expect("pending").insert(
            id,
            Pending {
                view: view.clone(),
                reply,
            },
        );
        let _waiting = Waiting { activity: self, id };
        let _ = self.inner.events.send(view);
        match tokio::time::timeout(ANSWER_WITHIN, answer).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Ok(Answer::Declined),
            Err(_) => Err(ApiError::conflict(
                "The person did not answer within 5 minutes. Nothing was changed.",
            )),
        }
    }

    fn resolve(&self, id: Uuid, answer: Answer) -> bool {
        let Some(pending) = self.inner.pending.lock().expect("pending").remove(&id) else {
            return false;
        };
        let _ = self
            .inner
            .events
            .send(json!({ "type": "resolved", "id": id }));
        pending.reply.send(answer).is_ok()
    }

    fn requires_key(&self, id: Uuid) -> Option<bool> {
        let pending = self.inner.pending.lock().expect("pending");
        let view = &pending.get(&id)?.view;
        Some(view["key"].is_object() && view["key"]["optional"] != true)
    }

    /// Tell the person what an agent changed. `message` is a template like
    /// `ask`'s title.
    pub fn notify(&self, who: Value, message: &str, values: Value, undo: Option<Undo>) {
        self.notify_with(who, message, values, undo, None);
    }

    /// A notice that also offers to open something, such as a session an
    /// agent just started.
    pub fn notify_with(
        &self,
        who: Value,
        message: &str,
        values: Value,
        undo: Option<Undo>,
        open: Option<Value>,
    ) {
        let id = Uuid::new_v4();
        let undoable = undo.is_some();
        if let Some(undo) = undo {
            let mut kept = self.inner.undo.lock().expect("undo");
            kept.push_back((id, undo));
            while kept.len() > NOTICES_KEPT {
                kept.pop_front();
            }
        }
        let _ = self.inner.events.send(json!({
            "type": "notice", "id": id, "who": who, "message": message, "values": values, "undoable": undoable, "open": open,
        }));
    }

    /// Point every open window at something: a file and line, a diff, a
    /// session. Nothing to point with no window open, and the agent is told.
    pub fn show(&self, who: Value, target: Value) -> Result<(), ApiError> {
        if !self.watched() {
            return Err(ApiError::conflict(
                "No AgentDock window is open to show this in.",
            ));
        }
        let _ = self
            .inner
            .events
            .send(json!({ "type": "show", "who": who, "target": target }));
        Ok(())
    }

    /// The shared canvas was changed on the server: windows load it.
    pub fn canvas_changed(&self, revision: u64) {
        let _ = self
            .inner
            .events
            .send(json!({ "type": "canvas", "revision": revision }));
    }

    fn take_undo(&self, id: Uuid) -> Option<Undo> {
        let mut kept = self.inner.undo.lock().expect("undo");
        let index = kept.iter().position(|(entry, _)| *entry == id)?;
        kept.remove(index).map(|(_, undo)| undo)
    }

    fn snapshot(&self) -> Value {
        let pending = self.inner.pending.lock().expect("pending");
        json!({ "type": "snapshot", "requests": pending.values().map(|p| p.view.clone()).collect::<Vec<_>>() })
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agent-activity/ws", get(socket))
        .route("/api/agent-activity/requests/{id}", post(answer))
        .route("/api/agent-activity/notices/{id}/undo", post(undo))
}

async fn socket(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| stream(state, socket))
}

async fn stream(state: AppState, mut socket: WebSocket) {
    let activity = state.activity.clone();
    let mut events = activity.inner.events.subscribe();
    if socket
        .send(Message::Text(activity.snapshot().to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            event = events.recv() => {
                let text = match event {
                    Ok(event) => event.to_string(),
                    // Fell behind: start again from what is pending now.
                    Err(broadcast::error::RecvError::Lagged(_)) => activity.snapshot().to_string(),
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerInput {
    approve: bool,
    #[serde(default)]
    secret: Option<String>,
}

async fn answer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<AnswerInput>,
) -> Result<Json<Value>, ApiError> {
    let secret = input
        .secret
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if input.approve && secret.is_none() && state.activity.requires_key(id) == Some(true) {
        return Err(ApiError::bad("Enter the key to approve this"));
    }
    let answer = if input.approve {
        Answer::Approved { secret }
    } else {
        Answer::Declined
    };
    if !state.activity.resolve(id, answer) {
        return Err(ApiError::missing("Request"));
    }
    Ok(Json(json!({ "answered": true })))
}

async fn undo(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let undo = state
        .activity
        .take_undo(id)
        .ok_or_else(|| ApiError::missing("Change to undo"))?;
    match undo {
        Undo::DeleteProfile { id, secret } => {
            db(&state, move |s| s.delete_endpoint_profile(id)).await?;
            if let Some(name) = secret {
                crate::secrets::remove(&state.state_dir, &name)?;
            }
        }
        Undo::RestoreProfile(profile) => {
            let profile = *profile;
            let id = profile.id;
            let exists = db(&state, move |s| s.get_endpoint_profile(id))
                .await?
                .is_some();
            db(&state, move |s| {
                if exists {
                    s.update_endpoint_profile(&profile).map(|_| ())
                } else {
                    s.create_endpoint_profile(&profile)
                }
            })
            .await?;
        }
        Undo::Preferences(value) => {
            db(&state, move |s| s.set_preferences(&value)).await?;
        }
        Undo::Canvas(layout) => {
            let current = db(&state, |s| s.get_shared_canvas_layout()).await?.revision;
            let store = state.store.clone();
            let revision = tokio::task::spawn_blocking(move || {
                crate::canvas::save_checked(&store, &layout, current)
            })
            .await
            .map_err(ApiError::internal)??
            .ok_or_else(|| {
                ApiError::conflict("The canvas changed again; undo is no longer possible")
            })?;
            state.activity.canvas_changed(revision);
        }
    }
    Ok(Json(json!({ "undone": true })))
}
