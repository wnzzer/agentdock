use super::*;
use agentdock_domain::InteractionMode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// A session name taken from the message that opened it.
///
/// Both clients used to leave a summary in their transcripts and no longer do —
/// there is not one `"type":"summary"` line left across the local Claude
/// history — so the first thing the user asked for is the only description of a
/// session that exists anywhere. It is what `claude --resume` falls back to for
/// the same reason.
///
/// Attachment paths and a slash command are stripped: a session called
/// `/model` describes every session. Returns `None` when nothing legible is
/// left, which keeps the created-at name rather than inventing one.
fn derived_title(text: &str) -> Option<String> {
    let body = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("@") && !line.starts_with('/'))?;
    // Count characters, not bytes: the limit is about how wide the sidebar row
    // reads, and CJK is where a byte limit would cut a name to a third.
    let mut title: String = body.chars().take(60).collect();
    if body.chars().count() > 60 {
        title.push('…');
    }
    Some(title).filter(|value| !value.is_empty())
}

pub struct StoredConversation {
    pub events: Vec<Value>,
    pub truncated: bool,
}
pub enum MessageSubmission {
    New(Value),
    Duplicate,
    Conflict,
}

impl Store {
    pub fn set_interaction_mode(&self, id: SessionId, mode: InteractionMode) -> Result<bool> {
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "UPDATE sessions SET interaction_mode=?1,updated_at=?2 WHERE id=?3 AND status IN ('stopped','failed') AND (provider!='terminal' OR ?1='pty')",
            params![if mode == InteractionMode::Structured {"structured"} else {"pty"}, Utc::now().to_rfc3339(), id.to_string()],
        )? == 1)
    }

    pub fn set_native_conversation_id(&self, id: SessionId, native_id: &str) -> Result<bool> {
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "UPDATE sessions SET provider_session_id=?1 WHERE id=?2 AND interaction_mode='structured'",
            params![native_id,id.to_string()],
        )? == 1)
    }

    /// A confirmed provider-local switch starts a fresh native context. Never
    /// carry credentials/environment from the previous account to its successor.
    pub fn switch_session_configuration(
        &self,
        id: SessionId,
        profile_id: Option<Uuid>,
        model: Option<String>,
    ) -> Result<bool> {
        self.switch_session_configuration_with_effort(id, profile_id, model, None)
    }

    pub fn switch_session_configuration_with_effort(
        &self,
        id: SessionId,
        profile_id: Option<Uuid>,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<bool> {
        let mut connection = self.connection.lock().expect("sqlite lock");
        let tx = connection.transaction()?;
        let session = tx.query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
            [id.to_string()],
            session_row,
        )?;
        if !matches!(
            session.status,
            SessionStatus::Stopped | SessionStatus::Failed
        ) || session.provider == ProviderKind::Terminal
        {
            return Ok(false);
        }
        let mut snapshot = profile_id
            .map(|pid| {
                tx.query_row(
                    &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE id=?1"),
                    [pid.to_string()],
                    profile_row,
                )
            })
            .transpose()?;
        if snapshot
            .as_ref()
            .is_some_and(|p| p.provider != session.provider)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if model.is_some() || effort.is_some() {
            let p = snapshot.get_or_insert_with(|| EndpointProfile {
                id: Uuid::new_v4(),
                provider: session.provider.clone(),
                name: "Session configuration".into(),
                endpoint_url: None,
                model: None,
                permission_mode: "native".into(),
                secret_ref: None,
                proxy_url: None,
                effort: None,
                model_aliases: Default::default(),
                native_config: None,
                environment: Default::default(),
                created_at: Utc::now(),
            });
            if let Some(model) = model {
                p.model = Some(model);
            }
            if let Some(effort) = effort {
                p.effort = Some(effort);
            }
        }
        let environment = snapshot
            .as_ref()
            .map(|p| p.environment.clone())
            .unwrap_or_default();
        agentdock_domain::validate_environment(&environment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let encoded = snapshot
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(conversion_error)?;
        tx.execute("UPDATE sessions SET endpoint_profile_id=?1,endpoint_snapshot=?2,environment=?3,provider_session_id=NULL,native_source_id=NULL,native_config_dir=NULL,configuration_revision=configuration_revision+1,updated_at=?4,error=NULL WHERE id=?5",
            params![profile_id.map(|v|v.to_string()),encoded,serde_json::to_string(&environment).map_err(conversion_error)?,Utc::now().to_rfc3339(),id.to_string()])?;
        append(
            &tx,
            id,
            json!({"type":"configuration","id":Uuid::new_v4().to_string(),"profile_name":snapshot.as_ref().map(|p|p.name.as_str()).unwrap_or("Isolated configuration"),"text":"A new native conversation will use this configuration. Earlier displayed history is not sent to the new endpoint."}),
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Point a stopped session at another checkout of its workspace's
    /// repository -- a worktree on another branch -- or back at the workspace
    /// itself with `None`. The next start is a different process in a different
    /// directory, so like an endpoint change it is a new native context: the
    /// revision is bumped, the native id cleared, and a boundary recorded.
    /// Returns false when the session is not stopped.
    pub fn set_session_checkout(
        &self,
        id: SessionId,
        path: Option<&str>,
        branch: Option<&str>,
        label: &str,
    ) -> Result<bool> {
        let mut connection = self.connection.lock().expect("sqlite lock");
        let tx = connection.transaction()?;
        let session = tx.query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
            [id.to_string()],
            session_row,
        )?;
        if !matches!(
            session.status,
            SessionStatus::Stopped | SessionStatus::Failed
        ) {
            return Ok(false);
        }
        tx.execute(
            "UPDATE sessions SET checkout_path=?1,checkout_branch=?2,provider_session_id=NULL,configuration_revision=configuration_revision+1,updated_at=?3,error=NULL WHERE id=?4",
            params![path, branch, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        append(
            &tx,
            id,
            json!({"type":"configuration","id":Uuid::new_v4().to_string(),"profile_name":label,"text":"The session now works in this checkout. A new native conversation starts there; earlier displayed history is not sent to it."}),
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn conversation(&self, id: SessionId) -> Result<StoredConversation> {
        let conn = self.connection.lock().expect("sqlite lock");
        let (truncated, anchors) = conn
            .query_row(
                "SELECT truncated,anchors FROM conversation_meta WHERE session_id=?1",
                [id.to_string()],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            .unwrap_or((false, "{}".into()));
        let mut statement = conn.prepare(
            "SELECT event_json FROM conversation_events WHERE session_id=?1 ORDER BY seq",
        )?;
        let mut events = statement
            .query_map([id.to_string()], |r| {
                serde_json::from_str::<Value>(&r.get::<_, String>(0)?).map_err(conversion_error)
            })?
            .collect::<Result<Vec<_>>>()?;
        let anchors: std::collections::BTreeMap<String, Value> =
            serde_json::from_str(&anchors).map_err(conversion_error)?;
        for event in anchors.into_values() {
            if !events
                .iter()
                .any(|existing| existing["seq"] == event["seq"])
            {
                events.push(event);
            }
        }
        events.sort_by_key(|event| event["seq"].as_u64().unwrap_or(0));
        Ok(StoredConversation { events, truncated })
    }

    pub fn append_conversation_event(&self, id: SessionId, event: Value) -> Result<Value> {
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction()?;
        let event = append(&tx, id, event)?;
        tx.commit()?;
        Ok(event)
    }

    pub fn chat_submission(
        &self,
        id: SessionId,
        message_id: &str,
        content: &str,
    ) -> Result<Option<bool>> {
        let hash:Option<String>=self.connection.lock().expect("sqlite lock").query_row("SELECT content_hash FROM conversation_submissions WHERE session_id=?1 AND message_id=?2",params![id.to_string(),message_id],|r|r.get(0)).optional()?;
        Ok(hash.map(|hash| hash == content_hash(content)))
    }

    /// Persist the accepted message and idempotency receipt atomically, before
    /// dispatch. An unknown transport outcome is never automatically replayed.
    pub fn submit_chat_message(
        &self,
        id: SessionId,
        message_id: &str,
        content: &str,
    ) -> Result<MessageSubmission> {
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction()?;
        let hash = content_hash(content);
        let previous:Option<String>=tx.query_row("SELECT content_hash FROM conversation_submissions WHERE session_id=?1 AND message_id=?2",params![id.to_string(),message_id],|r|r.get(0)).optional()?;
        if let Some(previous) = previous {
            return Ok(if previous == hash {
                MessageSubmission::Duplicate
            } else {
                MessageSubmission::Conflict
            });
        }
        tx.execute("INSERT INTO conversation_submissions(session_id,message_id,content_hash) VALUES (?1,?2,?3)",params![id.to_string(),message_id,hash])?;
        let event = append(
            &tx,
            id,
            json!({"type":"message","id":message_id,"role":"user","text":content,"delta":false}),
        )?;
        tx.commit()?;
        Ok(MessageSubmission::New(event))
    }
}

pub(super) fn append(
    tx: &rusqlite::Transaction<'_>,
    id: SessionId,
    mut event: Value,
) -> Result<Value> {
    if !event.is_object() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    tx.execute(
        "INSERT OR IGNORE INTO conversation_meta(session_id) VALUES (?1)",
        [id.to_string()],
    )?;
    let seq: u64 = tx.query_row(
        "SELECT next_seq FROM conversation_meta WHERE session_id=?1",
        [id.to_string()],
        |r| r.get(0),
    )?;
    event["seq"] = json!(seq);
    let encoded = serde_json::to_string(&event).map_err(conversion_error)?;
    if encoded.len() > 256 * 1024 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    tx.execute("INSERT INTO conversation_events(session_id,seq,event_json,byte_count) VALUES (?1,?2,?3,?4)",params![id.to_string(),seq,encoded,encoded.len()])?;
    tx.execute(
        "UPDATE conversation_meta SET next_seq=?1 WHERE session_id=?2",
        params![seq + 1, id.to_string()],
    )?;
    // The display window may rotate while a tool waits for input. Keep a
    // separate bounded set of exact native approval cards and current controls.
    let kind = event["type"].as_str().unwrap_or("");
    // The first thing asked names the session. Only a title this code wrote
    // itself is replaced, so a rename stays put, and only the first message
    // counts, so the name does not follow the conversation around.
    if kind == "message"
        && event["role"].as_str() == Some("user")
        && let Some(title) = event["text"].as_str().and_then(derived_title)
    {
        tx.execute(
            "UPDATE sessions SET title=?1,title_source='derived' WHERE id=?2 AND title_source='auto'",
            params![title, id.to_string()],
        )?;
    }
    if matches!(
        kind,
        "ready" | "turn" | "exit" | "configuration" | "usage" | "approval" | "approval_resolved"
    ) {
        let encoded_anchors: String = tx.query_row(
            "SELECT anchors FROM conversation_meta WHERE session_id=?1",
            [id.to_string()],
            |r| r.get(0),
        )?;
        let mut anchors: std::collections::BTreeMap<String, Value> =
            serde_json::from_str(&encoded_anchors).map_err(conversion_error)?;
        if kind == "configuration" {
            anchors.clear();
        }
        if kind == "exit" {
            anchors.retain(|key, _| !key.starts_with("approval:"));
        }
        if kind == "approval_resolved" {
            if let Some(request_id) = event["id"].as_str() {
                anchors.remove(&format!("approval:{request_id}"));
            }
        } else if kind == "approval" {
            let request_id = event["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
            anchors.insert(format!("approval:{request_id}"), event.clone());
            if anchors
                .keys()
                .filter(|key| key.starts_with("approval:"))
                .count()
                > 32
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        } else {
            anchors.insert(kind.into(), event.clone());
        }
        tx.execute(
            "UPDATE conversation_meta SET anchors=?1 WHERE session_id=?2",
            params![
                serde_json::to_string(&anchors).map_err(conversion_error)?,
                id.to_string()
            ],
        )?;
    }
    let mut stmt = tx.prepare(
        "SELECT seq,byte_count FROM conversation_events WHERE session_id=?1 ORDER BY seq DESC",
    )?;
    let rows = stmt.query_map([id.to_string()], |r| {
        Ok((r.get::<_, u64>(0)?, r.get::<_, usize>(1)?))
    })?;
    let mut bytes = 0;
    let mut cutoff = None;
    for (index, row) in rows.enumerate() {
        let (sequence, size) = row?;
        bytes += size;
        if index >= 2000 || bytes > 8 * 1024 * 1024 {
            cutoff = Some(sequence);
            break;
        }
    }
    if let Some(cutoff) = cutoff {
        tx.execute(
            "DELETE FROM conversation_events WHERE session_id=?1 AND seq<=?2",
            params![id.to_string(), cutoff],
        )?;
        tx.execute(
            "UPDATE conversation_meta SET truncated=1 WHERE session_id=?1",
            [id.to_string()],
        )?;
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transcript_receipts_and_sequence_survive_reopen_and_restart_does_not_replay_messages() {
        let path =
            std::env::temp_dir().join(format!("agentdock-conversation-{}.db", Uuid::new_v4()));
        let id;
        {
            let store = Store::open(&path).unwrap();
            let workspace = store.create_workspace("Fixture", "/fixture").unwrap();
            let session = store
                .create_session(workspace.id, ProviderKind::Codex, "Chat")
                .unwrap();
            id = session.id;
            assert_eq!(session.interaction_mode, InteractionMode::Pty);
            assert!(
                store
                    .set_interaction_mode(id, InteractionMode::Structured)
                    .unwrap()
            );
            assert!(matches!(
                store
                    .submit_chat_message(id, "message-1", "literal $PATH")
                    .unwrap(),
                MessageSubmission::New(_)
            ));
            store
                .append_conversation_event(id, json!({"type":"turn","status":"running"}))
                .unwrap();
            store.append_conversation_event(id,json!({"type":"approval","id":"pending","title":"Fixture","text":"Fixture","choices":["accept","decline"]})).unwrap();
            store
                .set_session_status(id, SessionStatus::Running)
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert!(matches!(
                store
                    .submit_chat_message(id, "message-1", "literal $PATH")
                    .unwrap(),
                MessageSubmission::Duplicate
            ));
            assert!(matches!(
                store
                    .submit_chat_message(id, "message-1", "changed")
                    .unwrap(),
                MessageSubmission::Conflict
            ));
            store.reconcile_after_restart().unwrap();
            let events = store.conversation(id).unwrap().events;
            assert_eq!(events.len(), 4);
            assert_eq!(events[3]["type"], "exit");
            assert_eq!(events[3]["seq"], 4);
            store.reconcile_after_restart().unwrap();
            assert_eq!(store.conversation(id).unwrap().events.len(), 4);
            assert!(matches!(
                store.get_session(id).unwrap().unwrap().status,
                SessionStatus::Stopped
            ));
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn transcript_retention_is_bounded_explicit_and_sequences_never_reset() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store.create_workspace("Fixture", "/fixture").unwrap();
        let id = store
            .create_session(workspace.id, ProviderKind::Codex, "Chat")
            .unwrap()
            .id;
        for _ in 0..2002 {
            store
                .append_conversation_event(id, json!({"type":"ready"}))
                .unwrap();
        }
        let transcript = store.conversation(id).unwrap();
        assert!(transcript.truncated);
        assert_eq!(transcript.events.len(), 2000);
        assert_eq!(transcript.events[0]["seq"], 3);
        assert_eq!(transcript.events.last().unwrap()["seq"], 2002);
        assert!(
            store
                .append_conversation_event(
                    id,
                    json!({"type":"error","message":"x".repeat(256*1024)})
                )
                .is_err()
        );
        assert_eq!(
            store.conversation(id).unwrap().events.last().unwrap()["seq"],
            2002
        );
    }
    #[test]
    fn pending_approvals_and_current_turn_survive_display_window_rotation() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store.create_workspace("Fixture", "/fixture").unwrap();
        let id = store
            .create_session(workspace.id, ProviderKind::Codex, "Chat")
            .unwrap()
            .id;
        store
            .append_conversation_event(id, json!({"type":"ready"}))
            .unwrap();
        store
            .append_conversation_event(id, json!({"type":"turn","status":"running"}))
            .unwrap();
        store.append_conversation_event(id,json!({"type":"approval","id":"pending","title":"Native approval","text":"Exact fixture request","choices":["accept","decline"]})).unwrap();
        for _ in 0..2001 {
            store.append_conversation_event(id,json!({"type":"tool","id":"progress","name":"Fixture progress","status":"running"})).unwrap();
        }
        let snapshot = store.conversation(id).unwrap();
        assert!(snapshot.truncated);
        assert_eq!(snapshot.events.len(), 2003);
        assert_eq!(snapshot.events[0]["type"], "ready");
        assert_eq!(snapshot.events[1]["status"], "running");
        assert_eq!(snapshot.events[2]["id"], "pending");
        store
            .append_conversation_event(id, json!({"type":"approval_resolved","id":"pending"}))
            .unwrap();
        assert!(
            !store
                .conversation(id)
                .unwrap()
                .events
                .iter()
                .any(|event| event["type"] == "approval")
        );
        let prompt = "prompt never duplicated into a receipt";
        store.submit_chat_message(id, "receipt", prompt).unwrap();
        let hash: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT content_hash FROM conversation_submissions WHERE message_id='receipt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(prompt));
        assert_eq!(
            store.chat_submission(id, "receipt", prompt).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn the_first_message_names_the_session_and_a_rename_is_never_overwritten() {
        let path = std::env::temp_dir().join(format!("agentdock-title-{}.db", Uuid::new_v4()));
        let store = Store::open(&path).unwrap();
        let workspace = store.create_workspace("Fixture", "/fixture").unwrap();
        let user = |text: &str| json!({"type":"message","role":"user","text":text});
        let named = |store: &Store, id| store.get_session(id).unwrap().unwrap().title;

        let first = store
            .create_session(workspace.id, ProviderKind::Codex, "Codex session")
            .unwrap()
            .id;
        // A leading slash command or file mention describes no session, so the
        // first legible line is what names it.
        store
            .append_conversation_event(first, user("/model"))
            .unwrap();
        assert_eq!(named(&store, first), "Codex session");
        store
            .append_conversation_event(first, user("@notes.md\nfix the retry backoff"))
            .unwrap();
        assert_eq!(named(&store, first), "fix the retry backoff");
        // Later messages are the conversation moving on, not a new name.
        store
            .append_conversation_event(first, user("and add a test"))
            .unwrap();
        assert_eq!(named(&store, first), "fix the retry backoff");

        // A name typed by hand outranks anything derived, before or after.
        let second = store
            .create_session(workspace.id, ProviderKind::ClaudeCode, "Claude session")
            .unwrap()
            .id;
        store
            .update_session_title(second, "Release checklist")
            .unwrap();
        store
            .append_conversation_event(second, user("look at the logs"))
            .unwrap();
        assert_eq!(named(&store, second), "Release checklist");

        // Long titles are cut by characters; a byte limit would cut CJK to a third.
        let third = store
            .create_session(workspace.id, ProviderKind::Codex, "Codex session")
            .unwrap()
            .id;
        store
            .append_conversation_event(third, user(&"\u{6d4b}".repeat(80)))
            .unwrap();
        let title = named(&store, third);
        assert_eq!(title.chars().count(), 61);
        assert!(title.ends_with('\u{2026}'));

        // Whitespace alone leaves the created-at name rather than a blank row.
        let fourth = store
            .create_session(workspace.id, ProviderKind::Codex, "Codex session")
            .unwrap()
            .id;
        store
            .append_conversation_event(fourth, user("   \n  "))
            .unwrap();
        assert_eq!(named(&store, fourth), "Codex session");
        let _ = std::fs::remove_file(&path);
    }
}
