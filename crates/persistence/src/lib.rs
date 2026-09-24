mod conversations;
pub use conversations::{MessageSubmission, StoredConversation};

use agentdock_domain::{
    EndpointProfile, EnvironmentOverrides, NativeConfigReference, ProviderKind, Session, SessionId,
    SessionStatus, Workspace, WorkspaceId,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Result, TransactionBehavior, params};
use std::{path::Path, sync::Mutex};
use uuid::Uuid;
pub type DbError = rusqlite::Error;

const SESSION_COLUMNS: &str = "id, workspace_id, provider, title, status, created_at, updated_at, endpoint_profile_id, provider_session_id, error, endpoint_snapshot, native_source_id, native_config_dir, environment, interaction_mode, configuration_revision, archived_at, ephemeral, resume_source_id, checkout_path, checkout_branch";
const PROFILE_COLUMNS: &str = "id, name, provider, endpoint_url, model, permission_mode, secret_ref, created_at, proxy_url, model_aliases, native_source_id, native_config_dir, native_config_env, environment, effort";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedCanvasLayout {
    pub layout: Option<String>,
    pub revision: u64,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 14 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let tx = connection.transaction()?;
        tx.execute_batch(include_str!("../../../migrations/0001_metadata.sql"))?;
        if version < 2 {
            tx.execute_batch(include_str!("../../../migrations/0002_endpoint_models.sql"))?;
        }
        // Upgrade the unversioned development schema without dropping any data.
        let names = {
            let mut stmt = tx.prepare("PRAGMA table_info(sessions)")?;
            stmt.query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<Vec<_>>>()?
        };
        for (name, definition) in [
            ("updated_at", "TEXT NOT NULL DEFAULT ''"),
            ("endpoint_profile_id", "TEXT"),
            ("provider_session_id", "TEXT"),
            ("error", "TEXT"),
            ("endpoint_snapshot", "TEXT"),
        ] {
            if !names.iter().any(|n| n == name) {
                tx.execute(
                    &format!("ALTER TABLE sessions ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
        tx.execute(
            "UPDATE sessions SET updated_at = created_at WHERE updated_at = ''",
            [],
        )?;
        if version < 3 {
            tx.execute_batch(include_str!("../../../migrations/0003_native_history.sql"))?;
        }
        if version < 4 {
            tx.execute_batch(include_str!("../../../migrations/0004_native_profiles.sql"))?;
        }
        if version < 5 {
            tx.execute_batch(include_str!("../../../migrations/0005_shared_canvas.sql"))?;
        }
        if version < 6 {
            tx.execute_batch(include_str!(
                "../../../migrations/0006_session_environment.sql"
            ))?;
        }
        if version < 7 {
            tx.execute_batch(include_str!("../../../migrations/0007_conversations.sql"))?;
        }
        if version < 8 {
            tx.execute_batch(include_str!("../../../migrations/0008_session_archive.sql"))?;
        }
        if version < 9 {
            tx.execute_batch(include_str!(
                "../../../migrations/0009_reasoning_effort.sql"
            ))?;
        }
        if version < 10 {
            tx.execute_batch(include_str!(
                "../../../migrations/0010_ephemeral_sessions.sql"
            ))?;
        }
        if version < 11 {
            tx.execute_batch(include_str!(
                "../../../migrations/0011_session_title_source.sql"
            ))?;
        }
        if version < 12 {
            tx.execute_batch(include_str!(
                "../../../migrations/0012_session_resume_source.sql"
            ))?;
        }
        if version < 13 {
            tx.execute_batch(include_str!(
                "../../../migrations/0013_session_checkout.sql"
            ))?;
        }
        if version < 14 {
            tx.execute_batch(include_str!("../../../migrations/0014_preferences.sql"))?;
        }
        // Capture the endpoint settings for legacy M0 sessions once, before templates change.
        let legacy = {
            let mut stmt = tx.prepare("SELECT s.id,p.id FROM sessions s JOIN endpoint_profiles p ON p.id=s.endpoint_profile_id WHERE s.endpoint_snapshot IS NULL")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>>>()?
        };
        for (id, profile_id) in legacy {
            let profile = tx.query_row(
                &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE id=?1"),
                [profile_id],
                profile_row,
            )?;
            let snapshot = serde_json::to_string(&profile).map_err(conversion_error)?;
            tx.execute(
                "UPDATE sessions SET endpoint_snapshot=?1 WHERE id=?2",
                params![snapshot, id],
            )?;
        }
        tx.pragma_update(None, "user_version", 14)?;
        tx.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn reconcile_after_restart(&self) -> Result<()> {
        let mut connection = self.connection.lock().expect("sqlite lock");
        let tx = connection.transaction()?;
        let ids = {
            let mut statement=tx.prepare("SELECT id FROM sessions WHERE interaction_mode='structured' AND status IN ('running','starting','waiting')")?;
            statement
                .query_map([], |r| parse_uuid(r.get(0)?))?
                .collect::<Result<Vec<_>>>()?
        };
        for id in ids {
            conversations::append(&tx, id, serde_json::json!({"type":"exit"}))?;
        }
        tx.execute(
            "UPDATE sessions SET status='stopped', error='Server restarted; reconnect history is unavailable. Restart the CLI and use its native resume command.', updated_at=?1 WHERE status IN ('running','starting','waiting')",
            [Utc::now().to_rfc3339()],
        )?;
        // A temporary session cannot outlive the process it was opened for, so
        // a restart clears it instead of leaving a stopped record behind. That
        // is the clutter the flag exists to avoid.
        tx.execute("DELETE FROM sessions WHERE ephemeral=1", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let conn = self.connection.lock().expect("sqlite lock");
        let mut stmt = conn.prepare(
            "SELECT id,name,root_path,created_at FROM workspaces ORDER BY created_at DESC,id",
        )?;
        stmt.query_map([], workspace_row)?.collect()
    }

    pub fn get_workspace(&self, id: WorkspaceId) -> Result<Option<Workspace>> {
        self.connection
            .lock()
            .expect("sqlite lock")
            .query_row(
                "SELECT id,name,root_path,created_at FROM workspaces WHERE id=?1",
                [id.to_string()],
                workspace_row,
            )
            .optional()
    }

    pub fn create_workspace(&self, name: &str, root_path: &str) -> Result<Workspace> {
        let conn = self.connection.lock().expect("sqlite lock");
        // Registering the same directory twice reopens its existing workspace.
        if let Some(existing) = conn
            .query_row(
                "SELECT id,name,root_path,created_at FROM workspaces WHERE root_path=?1 LIMIT 1",
                [root_path],
                workspace_row,
            )
            .optional()?
        {
            return Ok(existing);
        }
        let workspace = Workspace {
            id: Uuid::new_v4(),
            name: name.into(),
            root_path: root_path.into(),
            created_at: Utc::now(),
        };
        conn.execute(
            "INSERT INTO workspaces (id,name,root_path,created_at) VALUES (?1,?2,?3,?4)",
            params![
                workspace.id.to_string(),
                workspace.name,
                workspace.root_path,
                workspace.created_at.to_rfc3339()
            ],
        )?;
        Ok(workspace)
    }

    pub fn create_session(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
    ) -> Result<Session> {
        self.create_session_with_profile(workspace_id, provider, title, None)
    }

    pub fn create_session_with_profile(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        endpoint_profile_id: Option<Uuid>,
    ) -> Result<Session> {
        self.create_session_with_model(workspace_id, provider, title, endpoint_profile_id, None)
    }

    pub fn create_session_with_model(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        endpoint_profile_id: Option<Uuid>,
        model: Option<String>,
    ) -> Result<Session> {
        self.create_session_with_environment(
            workspace_id,
            provider,
            title,
            endpoint_profile_id,
            model,
            EnvironmentOverrides::new(),
        )
    }
    /// Discard a temporary session outright. Refusing anything else keeps the
    /// destructive path reachable only for sessions created as throwaway.
    /// Remove a session record. Only a temporary session, or a permanent one
    /// that has already been archived, can go: deleting is never one step away
    /// from a session still listed in its workspace.
    pub fn delete_session(&self, id: SessionId) -> Result<bool> {
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "DELETE FROM sessions WHERE id=?1 AND (ephemeral=1 OR archived_at IS NOT NULL)",
            [id.to_string()],
        )? > 0)
    }
    /// Update only the branch label of a session's checkout; the path it is
    /// bound to, and its native context, are unchanged.
    pub fn set_session_checkout_branch(&self, id: SessionId, branch: Option<&str>) -> Result<()> {
        self.connection.lock().expect("sqlite lock").execute(
            "UPDATE sessions SET checkout_branch=?1 WHERE id=?2 AND checkout_path IS NOT NULL",
            params![branch, id.to_string()],
        )?;
        Ok(())
    }
    /// Promote a temporary session to a permanent one. One-way on purpose.
    pub fn keep_session(&self, id: SessionId) -> Result<Option<Session>> {
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE sessions SET ephemeral=0,updated_at=?1 WHERE id=?2 AND ephemeral=1",
            params![Utc::now().to_rfc3339(), id.to_string()],
        )?;
        let session = tx
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
                [id.to_string()],
                session_row,
            )
            .optional()?;
        tx.commit()?;
        Ok(session)
    }

    pub fn create_session_with_environment(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        endpoint_profile_id: Option<Uuid>,
        model: Option<String>,
        overrides: EnvironmentOverrides,
    ) -> Result<Session> {
        self.create_session_with_configuration(
            workspace_id,
            provider,
            title,
            endpoint_profile_id,
            model,
            None,
            overrides,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session_with_configuration(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        endpoint_profile_id: Option<Uuid>,
        model: Option<String>,
        effort: Option<String>,
        overrides: EnvironmentOverrides,
        ephemeral: bool,
    ) -> Result<Session> {
        self.create_session_with_options(
            workspace_id,
            provider,
            title,
            endpoint_profile_id,
            model,
            effort,
            overrides,
            ephemeral,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session_with_options(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        endpoint_profile_id: Option<Uuid>,
        model: Option<String>,
        effort: Option<String>,
        overrides: EnvironmentOverrides,
        ephemeral: bool,
        resume_source_id: Option<SessionId>,
        provider_session_id: Option<String>,
    ) -> Result<Session> {
        agentdock_domain::validate_environment(&overrides)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction()?;
        let mut snapshot = if let Some(id) = endpoint_profile_id {
            let profile = tx.query_row(
                &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE id=?1"),
                [id.to_string()],
                profile_row,
            )?;
            // A session-level model/effort override is a launch flag, so it is
            // allowed even for a shared native configuration: it selects what
            // this session runs and never rewrites the client's own settings.
            if profile.provider != provider {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Some(profile)
        } else {
            None
        };
        let now = Utc::now();
        if model.is_some() || effort.is_some() {
            let profile = snapshot.get_or_insert_with(|| EndpointProfile {
                id: Uuid::new_v4(),
                name: "Native session".into(),
                provider: provider.clone(),
                endpoint_url: None,
                model: None,
                permission_mode: "native".into(),
                secret_ref: None,
                proxy_url: None,
                effort: None,
                model_aliases: Default::default(),
                native_config: None,
                environment: Default::default(),
                created_at: now,
            });
            if let Some(model) = model {
                profile.model = Some(model);
            }
            if let Some(effort) = effort {
                profile.effort = Some(effort);
            }
        }
        let mut environment = snapshot
            .as_ref()
            .map(|p| p.environment.clone())
            .unwrap_or_default();
        environment.extend(overrides);
        agentdock_domain::validate_environment(&environment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let environment_json = serde_json::to_string(&environment).map_err(conversion_error)?;
        let session = Session {
            interaction_mode: Default::default(),
            configuration_revision: 0,
            environment,
            id: Uuid::new_v4(),
            workspace_id,
            provider,
            title: title.into(),
            status: SessionStatus::Stopped,
            created_at: now,
            updated_at: now,
            archived_at: None,
            endpoint_profile_id,
            provider_session_id: provider_session_id.clone(),
            error: None,
            endpoint_snapshot: snapshot,
            native_source_id: None,
            native_config_dir: None,
            resume_source_id,
            checkout_path: None,
            checkout_branch: None,
            ephemeral,
        };
        let snapshot_json = session
            .endpoint_snapshot
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(conversion_error)?;
        // A created session starts with a placeholder name, so the first message
        // may replace it. An imported native session keeps the default 'manual':
        // its name came from the client's own history, not from this column.
        tx.execute("INSERT INTO sessions (id,workspace_id,provider,title,status,created_at,updated_at,endpoint_profile_id,endpoint_snapshot,environment,ephemeral,title_source,resume_source_id,provider_session_id) VALUES (?1,?2,?3,?4,?5,?6,?6,?7,?8,?9,?10,'auto',?11,?12)",
            params![session.id.to_string(),workspace_id.to_string(),provider_name(&session.provider),session.title,"stopped",now.to_rfc3339(),endpoint_profile_id.map(|v|v.to_string()),snapshot_json,environment_json,ephemeral,resume_source_id.map(|v|v.to_string()),provider_session_id])?;
        tx.commit()?;
        Ok(session)
    }

    pub fn list_sessions(&self, workspace_id: Option<WorkspaceId>) -> Result<Vec<Session>> {
        let conn = self.connection.lock().expect("sqlite lock");
        let mut stmt = conn.prepare(&format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE (?1 IS NULL OR workspace_id=?1) ORDER BY created_at DESC,id"))?;
        stmt.query_map([workspace_id.map(|v| v.to_string())], session_row)?
            .collect()
    }

    pub fn get_session(&self, id: SessionId) -> Result<Option<Session>> {
        self.connection
            .lock()
            .expect("sqlite lock")
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
                [id.to_string()],
                session_row,
            )
            .optional()
    }

    /// The stored preferences document, or `null` when none was ever saved.
    pub fn preferences(&self) -> Result<serde_json::Value> {
        let raw: Option<String> = self
            .connection
            .lock()
            .expect("sqlite lock")
            .query_row("SELECT value FROM preferences WHERE id=1", [], |r| r.get(0))
            .optional()?;
        match raw {
            Some(raw) => serde_json::from_str(&raw).map_err(conversion_error),
            None => Ok(serde_json::Value::Null),
        }
    }
    pub fn set_preferences(&self, value: &serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(value).map_err(conversion_error)?;
        self.connection.lock().expect("sqlite lock").execute(
            "INSERT INTO preferences(id,value,updated_at) VALUES (1,?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
            params![encoded, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
    pub fn update_session_title(&self, id: SessionId, title: &str) -> Result<Option<Session>> {
        let mut connection = self.connection.lock().expect("sqlite lock");
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            // A name typed by hand is never replaced by a derived one afterwards.
            "UPDATE sessions SET title=?1,title_source='manual',updated_at=?2 WHERE id=?3",
            params![title, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        let session = tx
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
                [id.to_string()],
                session_row,
            )
            .optional()?;
        tx.commit()?;
        Ok(session)
    }

    /// Archive is independent of process status, native resume identity and
    /// conversation history. Repeating the same request preserves timestamps.
    pub fn set_session_archived(&self, id: SessionId, archived: bool) -> Result<Option<Session>> {
        let mut connection = self.connection.lock().expect("sqlite lock");
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE sessions SET archived_at=CASE WHEN ?1 THEN ?2 ELSE NULL END,updated_at=?2 WHERE id=?3 AND ((?1 AND archived_at IS NULL) OR (NOT ?1 AND archived_at IS NOT NULL))",
            params![archived, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        let session = tx
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
                [id.to_string()],
                session_row,
            )
            .optional()?;
        tx.commit()?;
        Ok(session)
    }

    pub fn update_session_environment(
        &self,
        id: SessionId,
        environment: &EnvironmentOverrides,
    ) -> Result<bool> {
        agentdock_domain::validate_environment(environment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let encoded = serde_json::to_string(environment).map_err(conversion_error)?;
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "UPDATE sessions SET environment=?1,updated_at=?2 WHERE id=?3 AND status IN ('stopped','failed')",
            params![encoded,Utc::now().to_rfc3339(),id.to_string()],
        )? == 1)
    }

    pub fn set_session_status(&self, id: SessionId, status: SessionStatus) -> Result<bool> {
        self.set_session_result(id, status, None)
    }

    pub fn set_session_result(
        &self,
        id: SessionId,
        status: SessionStatus,
        error: Option<&str>,
    ) -> Result<bool> {
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "UPDATE sessions SET status=?1,error=?2,updated_at=?3 WHERE id=?4",
            params![
                status_name(&status),
                error,
                Utc::now().to_rfc3339(),
                id.to_string()
            ],
        )? == 1)
    }

    pub fn get_layout(&self, id: WorkspaceId) -> Result<Option<String>> {
        self.connection
            .lock()
            .expect("sqlite lock")
            .query_row(
                "SELECT layout_json FROM layouts WHERE workspace_id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn save_layout(&self, id: WorkspaceId, json: &str) -> Result<()> {
        self.connection.lock().expect("sqlite lock").execute("INSERT INTO layouts (workspace_id,layout_json,updated_at) VALUES (?1,?2,?3) ON CONFLICT(workspace_id) DO UPDATE SET layout_json=excluded.layout_json,updated_at=excluded.updated_at", params![id.to_string(),json,Utc::now().to_rfc3339()])?;
        Ok(())
    }

    /// Read only the singleton canvas. Legacy per-workspace layouts are never
    /// inspected or implicitly migrated by this operation.
    pub fn get_shared_canvas_layout(&self) -> Result<SharedCanvasLayout> {
        self.connection.lock().expect("sqlite lock").query_row(
            "SELECT layout_json,revision FROM shared_canvas WHERE singleton=1",
            [],
            |row| {
                Ok(SharedCanvasLayout {
                    layout: row.get(0)?,
                    revision: u64::try_from(row.get::<_, i64>(1)?).map_err(conversion_error)?,
                })
            },
        )
    }

    /// Compare-and-swap across tabs and independent SQLite connections. None
    /// means the caller's revision is stale; neither layout nor timestamp changes.
    pub fn save_shared_canvas_layout(
        &self,
        layout: &str,
        expected_revision: u64,
    ) -> Result<Option<u64>> {
        // SQLite stores signed 64-bit integers. Out-of-range expectations can
        // never match a writable revision, so report a normal CAS conflict.
        let Some(next_revision) = expected_revision
            .checked_add(1)
            .and_then(|value| i64::try_from(value).ok())
        else {
            return Ok(None);
        };
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE shared_canvas SET layout_json=?1,revision=?2,updated_at=?3 WHERE singleton=1 AND revision=?4",
            params![layout,next_revision,Utc::now().to_rfc3339(),expected_revision as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        tx.commit()?;
        Ok(Some(next_revision as u64))
    }

    pub fn list_endpoint_profiles(&self) -> Result<Vec<EndpointProfile>> {
        let conn = self.connection.lock().expect("sqlite lock");
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM endpoint_profiles ORDER BY created_at DESC"
        ))?;
        stmt.query_map([], profile_row)?.collect()
    }

    pub fn get_endpoint_profile(&self, id: Uuid) -> Result<Option<EndpointProfile>> {
        self.connection
            .lock()
            .expect("sqlite lock")
            .query_row(
                &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE id=?1"),
                [id.to_string()],
                profile_row,
            )
            .optional()
    }

    pub fn create_endpoint_profile(&self, p: &EndpointProfile) -> Result<()> {
        validate_native_profile(p)?;
        let aliases = serde_json::to_string(&p.model_aliases).map_err(conversion_error)?;
        self.connection.lock().expect("sqlite lock").execute("INSERT INTO endpoint_profiles (id,name,provider,endpoint_url,model,permission_mode,secret_ref,created_at,proxy_url,model_aliases,native_source_id,native_config_dir,native_config_env,environment,effort) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![p.id.to_string(),p.name,provider_name(&p.provider),p.endpoint_url,p.model,p.permission_mode,p.secret_ref,p.created_at.to_rfc3339(),p.proxy_url,aliases,p.native_config.as_ref().map(|native| &native.source_id),p.native_config.as_ref().map(|native| &native.config_dir),p.native_config.as_ref().and_then(|native| native.config_env.as_deref()),serde_json::to_string(&p.environment).map_err(conversion_error)?,p.effort])?;
        Ok(())
    }

    pub fn update_endpoint_profile(&self, p: &EndpointProfile) -> Result<bool> {
        validate_native_profile(p)?;
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(existing) = tx
            .query_row(
                &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE id=?1"),
                [p.id.to_string()],
                profile_row,
            )
            .optional()?
        else {
            return Ok(false);
        };
        // Native configuration belongs to its source client. A profile rename
        // is allowed; neither retargeting nor injecting overrides is allowed.
        // Ordinary profiles must use the import API to create a native reference.
        if existing.native_config != p.native_config
            || (existing.native_config.is_some() && existing.provider != p.provider)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let aliases = serde_json::to_string(&p.model_aliases).map_err(conversion_error)?;
        let updated = tx.execute("UPDATE endpoint_profiles SET name=?1,endpoint_url=?2,model=?3,permission_mode=?4,secret_ref=?5,proxy_url=?6,model_aliases=?7,environment=?9,effort=?10 WHERE id=?8",params![p.name,p.endpoint_url,p.model,p.permission_mode,p.secret_ref,p.proxy_url,aliases,p.id.to_string(),serde_json::to_string(&p.environment).map_err(conversion_error)?,p.effort])? == 1;
        tx.commit()?;
        Ok(updated)
    }

    /// Register a reference only: no source file is opened, copied or modified.
    /// IMMEDIATE serializes the lookup/insert across SQLite connections as well
    /// as this Store's mutex; the unique index is the final identity constraint.
    pub fn import_native_profile(
        &self,
        name: &str,
        provider: ProviderKind,
        reference: NativeConfigReference,
    ) -> Result<EndpointProfile> {
        let profile = EndpointProfile {
            id: Uuid::new_v4(),
            name: name.into(),
            provider,
            endpoint_url: None,
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: Some(reference),
            environment: Default::default(),
            created_at: Utc::now(),
        };
        validate_native_profile(&profile)?;
        let reference = profile
            .native_config
            .as_ref()
            .expect("native import reference");
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = tx.query_row(
            &format!("SELECT {PROFILE_COLUMNS} FROM endpoint_profiles WHERE provider=?1 AND native_source_id=?2 AND native_config_dir=?3 AND native_config_env IS ?4"),
            params![provider_name(&profile.provider),reference.source_id,reference.config_dir,reference.config_env],
            profile_row,
        ).optional()? {
            return Ok(existing);
        }
        tx.execute(
            "INSERT INTO endpoint_profiles (id,name,provider,permission_mode,created_at,model_aliases,native_source_id,native_config_dir,native_config_env) VALUES (?1,?2,?3,'native',?4,'{}',?5,?6,?7)",
            params![profile.id.to_string(),profile.name,provider_name(&profile.provider),profile.created_at.to_rfc3339(),reference.source_id,reference.config_dir,reference.config_env],
        )?;
        tx.commit()?;
        Ok(profile)
    }

    pub fn delete_endpoint_profile(&self, id: Uuid) -> Result<bool> {
        // Sessions retain their immutable profile snapshot even when a template is deleted.
        Ok(self.connection.lock().expect("sqlite lock").execute(
            "DELETE FROM endpoint_profiles WHERE id=?1",
            [id.to_string()],
        )? == 1)
    }
}

impl Store {
    pub fn import_native_session(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        source_id: &str,
        native_id: &str,
        config_dir: &str,
    ) -> Result<Session> {
        self.import_native_session_with_environment(
            workspace_id,
            provider,
            title,
            source_id,
            native_id,
            config_dir,
            None,
        )?
        .ok_or(rusqlite::Error::InvalidQuery)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn import_native_session_with_environment(
        &self,
        workspace_id: WorkspaceId,
        provider: ProviderKind,
        title: &str,
        source_id: &str,
        native_id: &str,
        config_dir: &str,
        environment: Option<EnvironmentOverrides>,
    ) -> Result<Option<Session>> {
        if let Some(environment) = &environment {
            agentdock_domain::validate_environment(environment)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
        }
        let mut conn = self.connection.lock().expect("sqlite lock");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing)=tx.query_row(&format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE workspace_id=?1 AND native_source_id=?2 AND provider_session_id=?3"),params![workspace_id.to_string(),source_id,native_id],session_row).optional()? {
            if environment.as_ref().is_some_and(|requested| *requested != existing.environment) { return Ok(None); }
            return Ok(Some(existing));
        }
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let encoded =
            serde_json::to_string(&environment.unwrap_or_default()).map_err(conversion_error)?;
        tx.execute("INSERT INTO sessions (id,workspace_id,provider,title,status,created_at,updated_at,provider_session_id,native_source_id,native_config_dir,environment) VALUES (?1,?2,?3,?4,'stopped',?5,?5,?6,?7,?8,?9)",params![id.to_string(),workspace_id.to_string(),provider_name(&provider),title,now,native_id,source_id,config_dir,encoded])?;
        let record = tx.query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id=?1"),
            [id.to_string()],
            session_row,
        )?;
        tx.commit()?;
        Ok(Some(record))
    }
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn validate_native_profile(profile: &EndpointProfile) -> Result<()> {
    agentdock_domain::validate_environment(&profile.environment)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let Some(reference) = &profile.native_config else {
        return Ok(());
    };
    if profile.provider == ProviderKind::Terminal
        || profile.name.trim().is_empty()
        || reference.source_id.trim().is_empty()
        || reference.config_dir.trim().is_empty()
        || reference.config_env.as_deref() == Some("")
        || profile.endpoint_url.is_some()
        || profile.model.is_some()
        || profile.secret_ref.is_some()
        // A proxy is allowed: it is the route AgentDock uses when it launches
        // the client, applied as launch environment. It never rewrites the
        // native configuration, unlike the settings rejected above.
        || !profile.model_aliases.is_empty()
        || profile.permission_mode != "native"
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}
fn parse_uuid(value: String) -> Result<Uuid> {
    Uuid::parse_str(&value).map_err(conversion_error)
}
fn parse_time(value: String) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(conversion_error)
}
fn parse_provider(value: String) -> Result<ProviderKind> {
    match value.as_str() {
        "claude_code" => Ok(ProviderKind::ClaudeCode),
        "codex" => Ok(ProviderKind::Codex),
        "terminal" => Ok(ProviderKind::Terminal),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn provider_name(p: &ProviderKind) -> &'static str {
    match p {
        ProviderKind::ClaudeCode => "claude_code",
        ProviderKind::Codex => "codex",
        ProviderKind::Terminal => "terminal",
    }
}
fn status_name(s: &SessionStatus) -> &'static str {
    match s {
        SessionStatus::Starting => "starting",
        SessionStatus::Running => "running",
        SessionStatus::Waiting => "waiting",
        SessionStatus::Stopped => "stopped",
        SessionStatus::Failed => "failed",
    }
}
fn workspace_row(r: &rusqlite::Row<'_>) -> Result<Workspace> {
    Ok(Workspace {
        id: parse_uuid(r.get(0)?)?,
        name: r.get(1)?,
        root_path: r.get(2)?,
        created_at: parse_time(r.get(3)?)?,
    })
}
fn profile_row(r: &rusqlite::Row<'_>) -> Result<EndpointProfile> {
    let native_config = match (
        r.get::<_, Option<String>>(10)?,
        r.get::<_, Option<String>>(11)?,
        r.get::<_, Option<String>>(12)?,
    ) {
        (Some(source_id), Some(config_dir), config_env) => Some(NativeConfigReference {
            source_id,
            config_dir,
            config_env,
        }),
        (None, None, None) => None,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(EndpointProfile {
        environment: serde_json::from_str(&r.get::<_, String>(13)?).map_err(conversion_error)?,
        id: parse_uuid(r.get(0)?)?,
        name: r.get(1)?,
        provider: parse_provider(r.get(2)?)?,
        endpoint_url: r.get(3)?,
        model: r.get(4)?,
        permission_mode: r.get(5)?,
        secret_ref: r.get(6)?,
        created_at: parse_time(r.get(7)?)?,
        proxy_url: r.get(8)?,
        effort: r.get(14)?,
        model_aliases: serde_json::from_str(&r.get::<_, String>(9)?).map_err(conversion_error)?,
        native_config,
    })
}
fn session_row(r: &rusqlite::Row<'_>) -> Result<Session> {
    let status = match r.get::<_, String>(4)?.as_str() {
        "starting" => SessionStatus::Starting,
        "running" => SessionStatus::Running,
        "waiting" => SessionStatus::Waiting,
        "stopped" => SessionStatus::Stopped,
        "failed" => SessionStatus::Failed,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Session {
        interaction_mode: match r.get::<_, String>(14)?.as_str() {
            "pty" => agentdock_domain::InteractionMode::Pty,
            "structured" => agentdock_domain::InteractionMode::Structured,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        configuration_revision: r.get(15)?,
        environment: serde_json::from_str(&r.get::<_, String>(13)?).map_err(conversion_error)?,
        native_source_id: r.get(11)?,
        native_config_dir: r.get(12)?,
        id: parse_uuid(r.get(0)?)?,
        workspace_id: parse_uuid(r.get(1)?)?,
        provider: parse_provider(r.get(2)?)?,
        title: r.get(3)?,
        status,
        created_at: parse_time(r.get(5)?)?,
        updated_at: parse_time(r.get(6)?)?,
        archived_at: r
            .get::<_, Option<String>>(16)?
            .map(parse_time)
            .transpose()?,
        ephemeral: r.get(17)?,
        resume_source_id: r
            .get::<_, Option<String>>(18)?
            .map(parse_uuid)
            .transpose()?,
        checkout_path: r.get(19)?,
        checkout_branch: r.get(20)?,
        endpoint_profile_id: r.get::<_, Option<String>>(7)?.map(parse_uuid).transpose()?,
        provider_session_id: r.get(8)?,
        error: r.get(9)?,
        endpoint_snapshot: r
            .get::<_, Option<String>>(10)?
            .map(|s| serde_json::from_str(&s).map_err(conversion_error))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdock_domain::EnvironmentValue;
    #[path = "archive_tests.rs"]
    mod archive_tests;

    #[test]
    fn environment_overrides_merge_snapshot_and_reopen_without_changing_templates() {
        let fixture = TempProfileDatabase::new();
        let session_id;
        let mut profile = ordinary_profile();
        profile.environment = [
            (
                "MODE".into(),
                EnvironmentValue::Literal {
                    value: "profile".into(),
                },
            ),
            (
                "KEEP".into(),
                EnvironmentValue::Literal {
                    value: "retained".into(),
                },
            ),
        ]
        .into();
        {
            let store = Store::open(&fixture.path).unwrap();
            let workspace = store
                .create_workspace("fixture", "/fixture/project")
                .unwrap();
            store.create_endpoint_profile(&profile).unwrap();
            let request = [
                ("MODE".into(), EnvironmentValue::Unset),
                (
                    "OPENAI_API_KEY".into(),
                    EnvironmentValue::SecretRef {
                        reference: "env:AGENTDOCK_SECRET_FIXTURE".into(),
                    },
                ),
            ]
            .into();
            let session = store
                .create_session_with_environment(
                    workspace.id,
                    ProviderKind::Codex,
                    "test",
                    Some(profile.id),
                    None,
                    request,
                )
                .unwrap();
            session_id = session.id;
            assert_eq!(session.environment["MODE"], EnvironmentValue::Unset);
            assert_eq!(session.environment["KEEP"], profile.environment["KEEP"]);
            assert_eq!(
                session.endpoint_snapshot.as_ref().unwrap().environment,
                profile.environment
            );
            profile.environment.clear();
            store.update_endpoint_profile(&profile).unwrap();
            assert_eq!(
                store
                    .get_session(session.id)
                    .unwrap()
                    .unwrap()
                    .environment
                    .len(),
                3
            );
            let native = store
                .import_native_profile("native", ProviderKind::Codex, fixture.reference())
                .unwrap();
            let mut changed = native.clone();
            changed.environment = [(
                "MODE".into(),
                EnvironmentValue::Literal {
                    value: "native-child".into(),
                },
            )]
            .into();
            store.update_endpoint_profile(&changed).unwrap();
            assert_eq!(
                store
                    .get_endpoint_profile(native.id)
                    .unwrap()
                    .unwrap()
                    .environment,
                changed.environment
            );
            store
                .set_session_status(session.id, SessionStatus::Running)
                .unwrap();
            assert!(
                !store
                    .update_session_environment(session.id, &EnvironmentOverrides::new())
                    .unwrap()
            );
            store
                .set_session_status(session.id, SessionStatus::Failed)
                .unwrap();
            assert!(
                store
                    .update_session_environment(
                        session.id,
                        &[(
                            "MODE".into(),
                            EnvironmentValue::Literal {
                                value: "patched".into()
                            }
                        )]
                        .into()
                    )
                    .unwrap()
            );
        }
        let store = Store::open(&fixture.path).unwrap();
        let session = store.get_session(session_id).unwrap().unwrap();
        assert_eq!(session.environment.len(), 1);
        assert_eq!(
            session.environment["MODE"],
            EnvironmentValue::Literal {
                value: "patched".into()
            }
        );
        assert_eq!(session.endpoint_snapshot.unwrap().environment.len(), 2);
    }

    #[test]
    fn history_environment_reimport_is_idempotent_and_never_changes_existing_session() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store
            .create_workspace("fixture", "/fixture/project")
            .unwrap();
        let environment: EnvironmentOverrides = [(
            "MODE".into(),
            EnvironmentValue::Literal {
                value: "first".into(),
            },
        )]
        .into();
        let session = store
            .import_native_session_with_environment(
                workspace.id,
                ProviderKind::Codex,
                "history",
                "source",
                "native-id",
                "/fixture/config",
                Some(environment.clone()),
            )
            .unwrap()
            .unwrap();
        store
            .set_session_status(session.id, SessionStatus::Running)
            .unwrap();
        assert_eq!(
            store
                .import_native_session(
                    workspace.id,
                    ProviderKind::Codex,
                    "again",
                    "source",
                    "native-id",
                    "/fixture/config"
                )
                .unwrap()
                .environment,
            environment
        );
        assert!(
            store
                .import_native_session_with_environment(
                    workspace.id,
                    ProviderKind::Codex,
                    "again",
                    "source",
                    "native-id",
                    "/fixture/config",
                    Some(EnvironmentOverrides::new())
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.get_session(session.id).unwrap().unwrap().environment,
            environment
        );
    }

    #[test]
    fn version_five_environment_migration_keeps_old_rows_and_snapshots_empty() {
        let fixture = TempProfileDatabase::new();
        let profile = ordinary_profile();
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let mut snapshot = serde_json::to_value(&profile).unwrap();
        snapshot.as_object_mut().unwrap().remove("environment");
        let snapshot = snapshot.to_string();
        {
            let conn = Connection::open(&fixture.path).unwrap();
            for migration in [
                include_str!("../../../migrations/0001_metadata.sql"),
                include_str!("../../../migrations/0002_endpoint_models.sql"),
                include_str!("../../../migrations/0003_native_history.sql"),
                include_str!("../../../migrations/0004_native_profiles.sql"),
                include_str!("../../../migrations/0005_shared_canvas.sql"),
            ] {
                conn.execute_batch(migration).unwrap();
            }
            conn.pragma_update(None, "user_version", 5).unwrap();
            conn.execute(
                "INSERT INTO workspaces VALUES (?1,'fixture','/fixture/project',?2)",
                params![workspace_id.to_string(), profile.created_at.to_rfc3339()],
            )
            .unwrap();
            conn.execute("INSERT INTO endpoint_profiles (id,name,provider,permission_mode,created_at) VALUES (?1,'fixture','codex','native',?2)",params![profile.id.to_string(),profile.created_at.to_rfc3339()]).unwrap();
            conn.execute("INSERT INTO sessions (id,workspace_id,provider,title,status,created_at,updated_at,endpoint_profile_id,endpoint_snapshot) VALUES (?1,?2,'codex','fixture','running',?3,?3,?4,?5)",params![session_id.to_string(),workspace_id.to_string(),profile.created_at.to_rfc3339(),profile.id.to_string(),snapshot]).unwrap();
        }
        for _ in 0..2 {
            let store = Store::open(&fixture.path).unwrap();
            assert!(
                store
                    .get_endpoint_profile(profile.id)
                    .unwrap()
                    .unwrap()
                    .environment
                    .is_empty()
            );
            let session = store.get_session(session_id).unwrap().unwrap();
            assert!(session.environment.is_empty());
            assert!(session.endpoint_snapshot.unwrap().environment.is_empty());
            assert!(matches!(session.status, SessionStatus::Running));
            assert_eq!(
                store
                    .connection
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT endpoint_snapshot FROM sessions WHERE id=?1",
                        [session_id.to_string()],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                snapshot
            );
        }
    }

    struct TempProfileDatabase {
        directory: std::path::PathBuf,
        path: std::path::PathBuf,
    }
    impl TempProfileDatabase {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("ad-native-profiles-{}", Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("metadata.db");
            Self { directory, path }
        }
        fn reference(&self) -> NativeConfigReference {
            NativeConfigReference {
                source_id: "fixture-codex".into(),
                // This directory is deliberately never created: persistence
                // must store a reference without opening client configuration.
                config_dir: self
                    .directory
                    .join("unopened-client-config")
                    .to_string_lossy()
                    .into_owned(),
                config_env: None,
            }
        }
    }
    impl Drop for TempProfileDatabase {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn ordinary_profile() -> EndpointProfile {
        EndpointProfile {
            id: Uuid::new_v4(),
            name: "Ordinary fixture".into(),
            provider: ProviderKind::Codex,
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
        }
    }

    #[test]
    fn resources_roundtrip_foreign_keys_snapshot() {
        let store = Store::open(":memory:").unwrap();
        let w = store.create_workspace("demo", "/tmp").unwrap();
        assert_eq!(
            store.create_workspace("duplicate", "/tmp").unwrap().id,
            w.id
        );
        assert!(
            store
                .create_session(Uuid::new_v4(), ProviderKind::Terminal, "bad")
                .is_err()
        );
        let mut p = EndpointProfile {
            id: Uuid::new_v4(),
            name: "one".into(),
            provider: ProviderKind::Codex,
            endpoint_url: None,
            model: Some("model-a".into()),
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: None,
            environment: Default::default(),
            created_at: Utc::now(),
        };
        store.create_endpoint_profile(&p).unwrap();
        let s = store
            .create_session_with_profile(w.id, ProviderKind::Codex, "review", Some(p.id))
            .unwrap();
        p.model = Some("model-b".into());
        store.update_endpoint_profile(&p).unwrap();
        store.delete_endpoint_profile(p.id).unwrap();
        assert_eq!(
            store
                .get_session(s.id)
                .unwrap()
                .unwrap()
                .endpoint_snapshot
                .unwrap()
                .model
                .as_deref(),
            Some("model-a")
        );
        store.save_layout(w.id, "{}").unwrap();
        assert_eq!(store.get_layout(w.id).unwrap().as_deref(), Some("{}"));
        store
            .set_session_status(s.id, SessionStatus::Running)
            .unwrap();
        store.reconcile_after_restart().unwrap();
        assert!(matches!(
            store.get_session(s.id).unwrap().unwrap().status,
            SessionStatus::Stopped
        ));
    }

    #[test]
    fn legacy_database_upgrades_without_data_loss() {
        let path = std::env::temp_dir().join(format!("ad-migration-{}.db", Uuid::new_v4()));
        let id = Uuid::new_v4().to_string();
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, workspace_id TEXT, provider TEXT, title TEXT, status TEXT, created_at TEXT);").unwrap();
            c.execute(
                "INSERT INTO sessions VALUES (?1,?1,'terminal','old','stopped',?2)",
                params![id, Utc::now().to_rfc3339()],
            )
            .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.list_sessions(None).unwrap().len(), 1);
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.list_sessions(None).unwrap()[0].title, "old");
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn native_history_import_is_idempotent_and_survives_database_reopen() {
        let directory = std::env::temp_dir().join(format!("ad-native-history-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("metadata.db");
        let (workspace_id, session_id);
        {
            let store = Store::open(&path).unwrap();
            let workspace = store
                .create_workspace("history fixture", "/fixture/repo")
                .unwrap();
            let imported = store
                .import_native_session(
                    workspace.id,
                    ProviderKind::Codex,
                    "Original thread",
                    "codex-fixture",
                    "thread_01",
                    "/fixture/private-codex-config",
                )
                .unwrap();
            assert!(matches!(imported.status, SessionStatus::Stopped));
            assert!(imported.endpoint_profile_id.is_none());
            assert!(imported.endpoint_snapshot.is_none());
            assert!(imported.error.is_none());

            let duplicate = store
                .import_native_session(
                    workspace.id,
                    ProviderKind::Codex,
                    "Changed title",
                    "codex-fixture",
                    "thread_01",
                    "/fixture/replacement-config",
                )
                .unwrap();
            assert_eq!(duplicate.id, imported.id);
            assert_eq!(duplicate.title, "Original thread");
            // Re-import must not silently retarget the original configuration
            // pin. The resume path guard remains responsible for rejecting it.
            assert_eq!(
                duplicate.native_config_dir.as_deref(),
                Some("/fixture/private-codex-config")
            );
            assert_eq!(store.list_sessions(Some(workspace.id)).unwrap().len(), 1);
            workspace_id = workspace.id;
            session_id = imported.id;
        }
        {
            let store = Store::open(&path).unwrap();
            let restored = store.get_session(session_id).unwrap().unwrap();
            assert_eq!(restored.native_source_id.as_deref(), Some("codex-fixture"));
            assert_eq!(restored.provider_session_id.as_deref(), Some("thread_01"));
            assert_eq!(
                restored.native_config_dir.as_deref(),
                Some("/fixture/private-codex-config")
            );
            assert!(matches!(restored.status, SessionStatus::Stopped));
            let duplicate = store
                .import_native_session(
                    workspace_id,
                    ProviderKind::Codex,
                    "Reopened",
                    "codex-fixture",
                    "thread_01",
                    "/fixture/private-codex-config",
                )
                .unwrap();
            assert_eq!(duplicate.id, session_id);
            assert_eq!(store.list_sessions(None).unwrap().len(), 1);
            assert!(
                store
                    .import_native_session(
                        Uuid::new_v4(),
                        ProviderKind::Codex,
                        "Unknown workspace",
                        "codex-fixture",
                        "thread_02",
                        "/fixture/private-codex-config",
                    )
                    .is_err()
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_history_identity_is_scoped_and_private_configuration_is_not_serialized() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store.create_workspace("fixture", "/fixture/repo").unwrap();
        let imported = store
            .import_native_session(
                workspace.id,
                ProviderKind::ClaudeCode,
                "A native conversation",
                "claude-one",
                "same-native-id",
                "/fixture/DO_NOT_EXPOSE_PRIVATE_CONFIG",
            )
            .unwrap();
        let other_source = store
            .import_native_session(
                workspace.id,
                ProviderKind::ClaudeCode,
                "Other account",
                "claude-two",
                "same-native-id",
                "/fixture/second-config",
            )
            .unwrap();
        assert_ne!(imported.id, other_source.id);
        let public = serde_json::to_value(&imported).unwrap();
        assert!(public.get("native_config_dir").is_none());
        assert!(!public.to_string().contains("DO_NOT_EXPOSE_PRIVATE_CONFIG"));
        assert_eq!(public["native_source_id"], "claude-one");
        assert_eq!(public["provider_session_id"], "same-native-id");
        // The secret-free public representation must not remove the internal
        // resume guard from persistence.
        assert_eq!(
            store
                .get_session(imported.id)
                .unwrap()
                .unwrap()
                .native_config_dir
                .as_deref(),
            Some("/fixture/DO_NOT_EXPOSE_PRIVATE_CONFIG")
        );
    }

    #[test]
    fn native_profiles_roundtrip_and_repeated_import_keep_identity_and_name() {
        let fixture = TempProfileDatabase::new();
        let reference = fixture.reference();
        let store = Store::open(":memory:").unwrap();
        let profile = store
            .import_native_profile("Existing Codex", ProviderKind::Codex, reference.clone())
            .unwrap();
        let duplicate = store
            .import_native_profile(
                "Do not rename on import",
                ProviderKind::Codex,
                reference.clone(),
            )
            .unwrap();
        assert_eq!(profile.id, duplicate.id);
        assert_eq!(duplicate.name, "Existing Codex");
        assert_eq!(duplicate.native_config, Some(reference.clone()));
        assert_eq!(duplicate.permission_mode, "native");
        assert!(duplicate.endpoint_url.is_none());
        assert!(duplicate.model.is_none());
        assert!(duplicate.secret_ref.is_none());
        assert!(duplicate.proxy_url.is_none());
        assert!(duplicate.model_aliases.is_empty());
        assert_eq!(
            store
                .get_endpoint_profile(profile.id)
                .unwrap()
                .unwrap()
                .native_config,
            Some(reference.clone())
        );
        assert_eq!(store.list_endpoint_profiles().unwrap().len(), 1);

        let public = serde_json::to_value(&profile).unwrap();
        assert_eq!(public["native_config"]["source_id"], reference.source_id);
        assert_eq!(public["native_config"]["config_dir"], reference.config_dir);
        assert!(!Path::new(&reference.config_dir).exists());
        let mut duplicate_insert = profile.clone();
        duplicate_insert.id = Uuid::new_v4();
        assert!(
            store.create_endpoint_profile(&duplicate_insert).is_err(),
            "SQLite must enforce duplicate native identity as well as the import method"
        );

        let mut other_source = reference.clone();
        other_source.source_id = "another-source".into();
        assert_ne!(
            store
                .import_native_profile("Other source", ProviderKind::Codex, other_source)
                .unwrap()
                .id,
            profile.id
        );
        let mut other_path = reference.clone();
        other_path.config_dir.push_str("-other");
        assert_ne!(
            store
                .import_native_profile("Other path", ProviderKind::Codex, other_path)
                .unwrap()
                .id,
            profile.id
        );
        assert_ne!(
            store
                .import_native_profile(
                    "Other provider",
                    ProviderKind::ClaudeCode,
                    reference.clone()
                )
                .unwrap()
                .id,
            profile.id
        );
        assert_eq!(store.list_endpoint_profiles().unwrap().len(), 4);
        assert!(
            store
                .import_native_profile("Terminal is unsupported", ProviderKind::Terminal, reference)
                .is_err()
        );
    }

    #[test]
    fn native_profiles_only_allow_name_updates_and_cannot_replace_ordinary_profiles() {
        let fixture = TempProfileDatabase::new();
        let reference = fixture.reference();
        let store = Store::open(":memory:").unwrap();
        let original = store
            .import_native_profile("Original", ProviderKind::Codex, reference.clone())
            .unwrap();
        for field in [
            "source",
            "directory",
            "provider",
            "endpoint",
            "model",
            "secret",
            "aliases",
            "permission",
            "remove-reference",
        ] {
            let mut changed = original.clone();
            match field {
                "source" => {
                    changed.native_config.as_mut().unwrap().source_id = "other-source".into()
                }
                "directory" => changed
                    .native_config
                    .as_mut()
                    .unwrap()
                    .config_dir
                    .push_str("-other"),
                "provider" => changed.provider = ProviderKind::ClaudeCode,
                "endpoint" => changed.endpoint_url = Some("https://example.invalid/v1".into()),
                "model" => changed.model = Some("override-model".into()),
                "secret" => changed.secret_ref = Some("env:AGENTDOCK_SECRET_FIXTURE".into()),
                "aliases" => {
                    changed
                        .model_aliases
                        .insert("fast".into(), "override-model".into());
                }
                "permission" => changed.permission_mode = "trusted".into(),
                "remove-reference" => changed.native_config = None,
                _ => unreachable!(),
            }
            assert!(
                store.update_endpoint_profile(&changed).is_err(),
                "Native {field} must not change through update"
            );
        }
        // A proxy is now allowed: it is the route AgentDock uses when launching
        // the client, not a native setting it rewrites.
        let mut proxied = original.clone();
        proxied.proxy_url = Some("http://127.0.0.1:7890".into());
        assert!(store.update_endpoint_profile(&proxied).unwrap());
        assert_eq!(
            store
                .get_endpoint_profile(original.id)
                .unwrap()
                .unwrap()
                .proxy_url
                .as_deref(),
            Some("http://127.0.0.1:7890")
        );

        let mut renamed = original.clone();
        renamed.name = "Personal Codex".into();
        assert!(store.update_endpoint_profile(&renamed).unwrap());
        let saved = store.get_endpoint_profile(original.id).unwrap().unwrap();
        assert_eq!(saved.name, "Personal Codex");
        assert_eq!(saved.native_config, Some(reference.clone()));
        assert_eq!(saved.created_at, original.created_at);

        let mut ordinary = ordinary_profile();
        store.create_endpoint_profile(&ordinary).unwrap();
        ordinary.native_config = Some(reference);
        assert!(store.update_endpoint_profile(&ordinary).is_err());
        assert!(
            store
                .get_endpoint_profile(ordinary.id)
                .unwrap()
                .unwrap()
                .native_config
                .is_none()
        );
    }

    #[test]
    fn native_profile_session_snapshots_survive_profile_rename_delete_and_database_reopen() {
        let fixture = TempProfileDatabase::new();
        let reference = fixture.reference();
        let (first_session, renamed_session, profile_id);
        {
            let store = Store::open(&fixture.path).unwrap();
            let workspace = store
                .create_workspace("fixture", "/fixture/workspace")
                .unwrap();
            let mut profile = store
                .import_native_profile("Original name", ProviderKind::Codex, reference.clone())
                .unwrap();
            profile_id = profile.id;
            first_session = store
                .create_session_with_profile(
                    workspace.id,
                    ProviderKind::Codex,
                    "first",
                    Some(profile.id),
                )
                .unwrap()
                .id;
            profile.name = "Renamed profile".into();
            store.update_endpoint_profile(&profile).unwrap();
            renamed_session = store
                .create_session_with_profile(
                    workspace.id,
                    ProviderKind::Codex,
                    "second",
                    Some(profile.id),
                )
                .unwrap()
                .id;
            assert!(store.delete_endpoint_profile(profile.id).unwrap());
            assert!(store.list_endpoint_profiles().unwrap().is_empty());
        }
        {
            let store = Store::open(&fixture.path).unwrap();
            let first = store.get_session(first_session).unwrap().unwrap();
            let second = store.get_session(renamed_session).unwrap().unwrap();
            assert_eq!(first.endpoint_profile_id, Some(profile_id));
            assert_eq!(second.endpoint_profile_id, Some(profile_id));
            assert!(
                first.native_source_id.is_none(),
                "Profile binding is separate from native history import"
            );
            assert!(first.native_config_dir.is_none());
            let first_snapshot = first.endpoint_snapshot.unwrap();
            let second_snapshot = second.endpoint_snapshot.unwrap();
            assert_eq!(first_snapshot.name, "Original name");
            assert_eq!(second_snapshot.name, "Renamed profile");
            assert_eq!(first_snapshot.native_config, Some(reference.clone()));
            assert_eq!(second_snapshot.native_config, Some(reference.clone()));
            assert!(store.get_endpoint_profile(profile_id).unwrap().is_none());
            assert!(!Path::new(&reference.config_dir).exists());
        }
    }

    #[test]
    fn native_profile_rejects_session_model_override_without_changing_legacy_behavior() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store
            .create_workspace("fixture", "/fixture/workspace")
            .unwrap();
        let reference = NativeConfigReference {
            source_id: "fixture".into(),
            config_dir: "/fixture/native-config".into(),
            config_env: None,
        };
        let profile = store
            .import_native_profile("Native", ProviderKind::Codex, reference.clone())
            .unwrap();
        // A session-level model is a launch flag: it selects what this session
        // runs without rewriting the shared native configuration, so it is kept
        // on the session snapshot while the imported profile stays untouched.
        let overridden = store
            .create_session_with_model(
                workspace.id,
                ProviderKind::Codex,
                "overridden",
                Some(profile.id),
                Some("override-model".into()),
            )
            .unwrap();
        let snapshot = overridden.endpoint_snapshot.unwrap();
        assert_eq!(snapshot.model.as_deref(), Some("override-model"));
        assert_eq!(snapshot.native_config, Some(reference.clone()));
        assert_eq!(
            store
                .get_endpoint_profile(profile.id)
                .unwrap()
                .unwrap()
                .model,
            None,
            "the imported profile itself still carries no model"
        );
        let native = store
            .create_session_with_model(
                workspace.id,
                ProviderKind::Codex,
                "native",
                Some(profile.id),
                None,
            )
            .unwrap();
        assert_eq!(
            native.endpoint_snapshot.unwrap().native_config,
            Some(reference)
        );

        let ordinary = ordinary_profile();
        store.create_endpoint_profile(&ordinary).unwrap();
        let customized = store
            .create_session_with_model(
                workspace.id,
                ProviderKind::Codex,
                "customized",
                Some(ordinary.id),
                Some("override-model".into()),
            )
            .unwrap();
        let snapshot = customized.endpoint_snapshot.unwrap();
        assert_eq!(snapshot.model.as_deref(), Some("override-model"));
        assert!(snapshot.native_config.is_none());
        let unbound = store
            .create_session_with_model(
                workspace.id,
                ProviderKind::Codex,
                "unbound",
                None,
                Some("native-default-override".into()),
            )
            .unwrap();
        assert!(unbound.endpoint_snapshot.unwrap().native_config.is_none());
    }

    #[test]
    fn version_three_migration_preserves_profiles_and_legacy_snapshots() {
        let fixture = TempProfileDatabase::new();
        let profile = ordinary_profile();
        let workspace_id = Uuid::new_v4();
        let snapshot_session_id = Uuid::new_v4();
        let backfill_session_id = Uuid::new_v4();
        let mut legacy_snapshot = serde_json::to_value(&profile).unwrap();
        legacy_snapshot
            .as_object_mut()
            .unwrap()
            .remove("native_config");
        {
            let conn = Connection::open(&fixture.path).unwrap();
            conn.execute_batch(include_str!("../../../migrations/0001_metadata.sql"))
                .unwrap();
            conn.execute_batch(include_str!("../../../migrations/0002_endpoint_models.sql"))
                .unwrap();
            conn.execute_batch(include_str!("../../../migrations/0003_native_history.sql"))
                .unwrap();
            conn.pragma_update(None, "user_version", 3).unwrap();
            conn.execute(
                "INSERT INTO workspaces VALUES (?1,'fixture','/fixture/workspace',?2)",
                params![workspace_id.to_string(), profile.created_at.to_rfc3339()],
            )
            .unwrap();
            conn.execute("INSERT INTO endpoint_profiles (id,name,provider,permission_mode,created_at,proxy_url,model_aliases) VALUES (?1,?2,'codex','native',?3,'http://127.0.0.1:7890','{\"fast\":\"model-id\"}')",params![profile.id.to_string(),profile.name,profile.created_at.to_rfc3339()]).unwrap();
            for (id, snapshot) in [
                (snapshot_session_id, Some(legacy_snapshot.to_string())),
                (backfill_session_id, None),
            ] {
                conn.execute("INSERT INTO sessions (id,workspace_id,provider,title,status,created_at,updated_at,endpoint_profile_id,endpoint_snapshot) VALUES (?1,?2,'codex','legacy','running',?3,?3,?4,?5)",params![id.to_string(),workspace_id.to_string(),profile.created_at.to_rfc3339(),profile.id.to_string(),snapshot]).unwrap();
            }
        }
        for _ in 0..2 {
            let store = Store::open(&fixture.path).unwrap();
            assert_eq!(
                store
                    .connection
                    .lock()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                14
            );
            let current = store.get_endpoint_profile(profile.id).unwrap().unwrap();
            assert!(current.native_config.is_none());
            assert_eq!(current.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
            assert_eq!(current.model_aliases["fast"], "model-id");
            let legacy = store.get_session(snapshot_session_id).unwrap().unwrap();
            assert!(
                matches!(legacy.status, SessionStatus::Running),
                "Migration must not reconcile running sessions"
            );
            assert!(legacy.endpoint_snapshot.unwrap().native_config.is_none());
            let backfilled = store
                .get_session(backfill_session_id)
                .unwrap()
                .unwrap()
                .endpoint_snapshot
                .unwrap();
            assert!(backfilled.native_config.is_none());
            assert_eq!(backfilled.model_aliases["fast"], "model-id");
            assert_eq!(
                backfilled.proxy_url.as_deref(),
                Some("http://127.0.0.1:7890")
            );
            let imported = store
                .import_native_profile("Native fixture", ProviderKind::Codex, fixture.reference())
                .unwrap();
            assert!(imported.native_config.is_some());
        }
    }

    #[test]
    fn concurrent_native_profile_imports_across_stores_return_the_same_profile() {
        use std::sync::{Arc, Barrier};
        let fixture = TempProfileDatabase::new();
        let stores: Vec<_> = (0..4)
            .map(|_| Store::open(&fixture.path).unwrap())
            .collect();
        let barrier = Arc::new(Barrier::new(stores.len()));
        let handles: Vec<_> = stores
            .into_iter()
            .map(|store| {
                let barrier = barrier.clone();
                let reference = fixture.reference();
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .import_native_profile("Shared import", ProviderKind::Codex, reference)
                        .unwrap()
                        .id
                })
            })
            .collect();
        let ids: Vec<_> = handles
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        let store = Store::open(&fixture.path).unwrap();
        assert_eq!(store.list_endpoint_profiles().unwrap().len(), 1);
        assert!(!Path::new(&fixture.reference().config_dir).exists());
    }

    #[test]
    fn native_profile_identity_preserves_unset_vs_explicit_config_environment() {
        let fixture = TempProfileDatabase::new();
        let store = Store::open(":memory:").unwrap();
        let implicit = fixture.reference();
        let mut explicit = implicit.clone();
        explicit.config_env = Some(implicit.config_dir.clone());
        let default_profile = store
            .import_native_profile(
                "Default native context",
                ProviderKind::Codex,
                implicit.clone(),
            )
            .unwrap();
        let explicit_profile = store
            .import_native_profile(
                "Explicit native context",
                ProviderKind::Codex,
                explicit.clone(),
            )
            .unwrap();
        assert_ne!(
            default_profile.id, explicit_profile.id,
            "Setting the same directory explicitly may change the native client's credential identity"
        );
        for (reference, expected_id) in [
            (implicit.clone(), default_profile.id),
            (explicit.clone(), explicit_profile.id),
        ] {
            let repeated = store
                .import_native_profile("Do not rename", ProviderKind::Codex, reference)
                .unwrap();
            assert_eq!(repeated.id, expected_id);
        }
        let mut changed_default = default_profile.clone();
        changed_default.native_config = Some(explicit.clone());
        assert!(store.update_endpoint_profile(&changed_default).is_err());
        let mut changed_explicit = explicit_profile.clone();
        changed_explicit.native_config = Some(implicit);
        assert!(store.update_endpoint_profile(&changed_explicit).is_err());
        for profile in [&default_profile, &explicit_profile] {
            let mut duplicate = profile.clone();
            duplicate.id = Uuid::new_v4();
            assert!(
                store.create_endpoint_profile(&duplicate).is_err(),
                "Both NULL and explicit environment identities must be unique"
            );
        }
        let mut empty = explicit;
        empty.config_env = Some(String::new());
        assert!(
            store
                .import_native_profile("Ambiguous empty value", ProviderKind::Codex, empty.clone())
                .is_err()
        );
        let mut empty_profile = explicit_profile;
        empty_profile.id = Uuid::new_v4();
        empty_profile.native_config = Some(empty);
        assert!(store.create_endpoint_profile(&empty_profile).is_err());
        assert_eq!(store.list_endpoint_profiles().unwrap().len(), 2);
    }

    #[test]
    fn native_config_environment_snapshots_keep_original_strings_after_delete_and_reopen() {
        let fixture = TempProfileDatabase::new();
        let implicit = fixture.reference();
        let mut explicit = implicit.clone();
        explicit.config_env = Some(implicit.config_dir.clone());
        let mut original_spelling = implicit.clone();
        original_spelling.config_env = Some(format!(
            "{}/../unopened-client-config/",
            implicit.config_dir
        ));
        let references = [implicit, explicit, original_spelling];
        let mut session_ids = Vec::new();
        {
            let store = Store::open(&fixture.path).unwrap();
            let workspace = store
                .create_workspace("Environment fixture", "/fixture/workspace")
                .unwrap();
            let mut profile_ids = Vec::new();
            for reference in &references {
                let profile = store
                    .import_native_profile("Native fixture", ProviderKind::Codex, reference.clone())
                    .unwrap();
                profile_ids.push(profile.id);
                let roundtrip = store.get_endpoint_profile(profile.id).unwrap().unwrap();
                assert_eq!(roundtrip.native_config.as_ref(), Some(reference));
                let session = store
                    .create_session_with_profile(
                        workspace.id,
                        ProviderKind::Codex,
                        "Saved context",
                        Some(profile.id),
                    )
                    .unwrap();
                session_ids.push(session.id);
            }
            assert_ne!(profile_ids[0], profile_ids[1]);
            assert_ne!(profile_ids[1], profile_ids[2]);
            assert_eq!(store.list_endpoint_profiles().unwrap().len(), 3);
            for profile_id in profile_ids {
                assert!(store.delete_endpoint_profile(profile_id).unwrap());
            }
        }
        {
            let store = Store::open(&fixture.path).unwrap();
            for (session_id, reference) in session_ids.into_iter().zip(&references) {
                let snapshot = store
                    .get_session(session_id)
                    .unwrap()
                    .unwrap()
                    .endpoint_snapshot
                    .unwrap();
                assert_eq!(snapshot.native_config.as_ref(), Some(reference));
                let public = serde_json::to_value(&snapshot).unwrap();
                assert_eq!(
                    public["native_config"]["config_env"],
                    serde_json::to_value(&reference.config_env).unwrap()
                );
            }
            assert!(store.list_endpoint_profiles().unwrap().is_empty());
            assert!(!Path::new(&references[0].config_dir).exists());
        }
    }

    #[test]
    fn native_config_snapshot_without_environment_field_defaults_to_unset() {
        let fixture = TempProfileDatabase::new();
        let store = Store::open(":memory:").unwrap();
        let profile = store
            .import_native_profile("Legacy reference", ProviderKind::Codex, fixture.reference())
            .unwrap();
        let mut snapshot = serde_json::to_value(&profile).unwrap();
        snapshot["native_config"]
            .as_object_mut()
            .unwrap()
            .remove("config_env");
        let decoded: EndpointProfile = serde_json::from_value(snapshot).unwrap();
        assert!(decoded.native_config.unwrap().config_env.is_none());
    }

    #[test]
    fn shared_canvas_is_initially_empty_and_uses_compare_and_swap_revisions() {
        let store = Store::open(":memory:").unwrap();
        let workspace = store
            .create_workspace("legacy fixture", "/fixture/legacy")
            .unwrap();
        store
            .save_layout(workspace.id, "legacy layout is not consulted")
            .unwrap();
        assert_eq!(
            store.get_shared_canvas_layout().unwrap(),
            SharedCanvasLayout {
                layout: None,
                revision: 0
            }
        );
        assert_eq!(
            store.save_shared_canvas_layout("first", 0).unwrap(),
            Some(1)
        );
        let saved_at: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT updated_at FROM shared_canvas WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(store.save_shared_canvas_layout("stale", 0).unwrap(), None);
        assert_eq!(
            store.get_shared_canvas_layout().unwrap(),
            SharedCanvasLayout {
                layout: Some("first".into()),
                revision: 1
            }
        );
        assert_eq!(
            store
                .connection
                .lock()
                .unwrap()
                .query_row(
                    "SELECT updated_at FROM shared_canvas WHERE singleton=1",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            saved_at
        );
        assert_eq!(
            store.save_shared_canvas_layout("second", 1).unwrap(),
            Some(2)
        );
        assert_eq!(
            store
                .save_shared_canvas_layout("out of range", u64::MAX)
                .unwrap(),
            None
        );
        assert_eq!(
            store.get_shared_canvas_layout().unwrap().layout.as_deref(),
            Some("second")
        );
        assert_eq!(
            store.get_layout(workspace.id).unwrap().as_deref(),
            Some("legacy layout is not consulted")
        );
        assert!(
            store
                .connection
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO shared_canvas (singleton,revision,updated_at) VALUES (2,0,'test')",
                    []
                )
                .is_err()
        );
    }

    #[test]
    fn version_four_shared_canvas_migration_leaves_legacy_resources_unchanged() {
        let fixture = TempProfileDatabase::new();
        let mut profile = ordinary_profile();
        profile.native_config = Some(fixture.reference());
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let old_layout = "{opaque-legacy-layout:do-not-parse-or-merge}";
        let old_snapshot = serde_json::to_string(&profile).unwrap();
        {
            let conn = Connection::open(&fixture.path).unwrap();
            for migration in [
                include_str!("../../../migrations/0001_metadata.sql"),
                include_str!("../../../migrations/0002_endpoint_models.sql"),
                include_str!("../../../migrations/0003_native_history.sql"),
                include_str!("../../../migrations/0004_native_profiles.sql"),
            ] {
                conn.execute_batch(migration).unwrap();
            }
            conn.pragma_update(None, "user_version", 4).unwrap();
            conn.execute(
                "INSERT INTO workspaces VALUES (?1,'unchanged workspace','/fixture/project',?2)",
                params![workspace_id.to_string(), profile.created_at.to_rfc3339()],
            )
            .unwrap();
            let reference = profile.native_config.as_ref().unwrap();
            conn.execute("INSERT INTO endpoint_profiles (id,name,provider,permission_mode,created_at,native_source_id,native_config_dir,native_config_env) VALUES (?1,?2,'codex','native',?3,?4,?5,?6)", params![profile.id.to_string(),profile.name,profile.created_at.to_rfc3339(),reference.source_id,reference.config_dir,reference.config_env]).unwrap();
            conn.execute("INSERT INTO sessions (id,workspace_id,provider,title,status,created_at,updated_at,endpoint_profile_id,endpoint_snapshot,error) VALUES (?1,?2,'codex','unchanged session','running',?3,?3,?4,?5,'unchanged diagnostic')", params![session_id.to_string(),workspace_id.to_string(),profile.created_at.to_rfc3339(),profile.id.to_string(),old_snapshot]).unwrap();
            conn.execute(
                "INSERT INTO layouts VALUES (?1,?2,'unchanged timestamp')",
                params![workspace_id.to_string(), old_layout],
            )
            .unwrap();
        }
        {
            let store = Store::open(&fixture.path).unwrap();
            assert_eq!(
                store.get_shared_canvas_layout().unwrap(),
                SharedCanvasLayout {
                    layout: None,
                    revision: 0
                }
            );
            assert_eq!(
                store.get_layout(workspace_id).unwrap().as_deref(),
                Some(old_layout)
            );
            assert_eq!(
                store
                    .get_endpoint_profile(profile.id)
                    .unwrap()
                    .unwrap()
                    .native_config,
                profile.native_config
            );
            let session = store.get_session(session_id).unwrap().unwrap();
            assert!(matches!(session.status, SessionStatus::Running));
            assert_eq!(session.title, "unchanged session");
            assert_eq!(session.error.as_deref(), Some("unchanged diagnostic"));
            assert_eq!(
                serde_json::to_string(&session.endpoint_snapshot.unwrap()).unwrap(),
                old_snapshot
            );
            assert_eq!(
                store
                    .save_shared_canvas_layout("explicit shared layout", 0)
                    .unwrap(),
                Some(1)
            );
        }
        {
            let store = Store::open(&fixture.path).unwrap();
            assert_eq!(
                store.get_shared_canvas_layout().unwrap(),
                SharedCanvasLayout {
                    layout: Some("explicit shared layout".into()),
                    revision: 1
                }
            );
            assert_eq!(
                store.get_layout(workspace_id).unwrap().as_deref(),
                Some(old_layout)
            );
            assert_eq!(
                store
                    .connection
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT updated_at FROM layouts WHERE workspace_id=?1",
                        [workspace_id.to_string()],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                "unchanged timestamp"
            );
            assert_eq!(
                store
                    .connection
                    .lock()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                14
            );
            assert!(matches!(
                store.get_session(session_id).unwrap().unwrap().status,
                SessionStatus::Running
            ));
            assert!(!Path::new(&fixture.reference().config_dir).exists());
        }
    }

    #[test]
    fn shared_canvas_concurrent_writers_cannot_overwrite_the_winning_revision() {
        use std::sync::{Arc, Barrier};
        let fixture = TempProfileDatabase::new();
        let stores: Vec<_> = (0..4)
            .map(|_| Store::open(&fixture.path).unwrap())
            .collect();
        let barrier = Arc::new(Barrier::new(stores.len()));
        let handles: Vec<_> = stores
            .into_iter()
            .enumerate()
            .map(|(index, store)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let layout = format!("candidate-{index}");
                    barrier.wait();
                    let result = store.save_shared_canvas_layout(&layout, 0).unwrap();
                    (layout, result)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let winners: Vec<_> = results
            .iter()
            .filter(|(_, revision)| *revision == Some(1))
            .collect();
        assert_eq!(winners.len(), 1);
        let store = Store::open(&fixture.path).unwrap();
        assert_eq!(
            store.get_shared_canvas_layout().unwrap(),
            SharedCanvasLayout {
                layout: Some(winners[0].0.clone()),
                revision: 1
            }
        );
    }
}
