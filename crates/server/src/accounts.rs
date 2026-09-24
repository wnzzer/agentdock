//! Account metadata plus official native-client account operations. Credential
//! contents are never parsed, copied into profiles, or returned by this module.
use crate::{ApiError, AppState, Result};
use agentdock_domain::{EndpointProfile, NativeConfigReference, ProviderKind, SessionStatus};
use agentdock_runtime::{SpawnSpec, process::OwnedGroup};
use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
    task::JoinHandle,
};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct AccountManager {
    inner: Arc<Mutex<ManagerState>>,
    metadata: Arc<Mutex<()>>,
}
#[derive(Default)]
struct ManagerState {
    views: HashMap<Uuid, AccountView>,
    generations: HashMap<Uuid, Uuid>,
    pending: HashMap<Uuid, PendingLogin>,
    operations: HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>,
    maintenance: HashSet<Uuid>,
}
struct PendingLogin {
    generation: Uuid,
    cancel: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl AccountManager {
    pub fn new() -> Self {
        Self::default()
    }
    fn operation(&self, id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.inner
            .lock()
            .expect("accounts lock")
            .operations
            .entry(id)
            .or_default()
            .clone()
    }
    fn pending(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .expect("accounts lock")
            .pending
            .contains_key(&id)
    }
    pub async fn shutdown(&self) {
        let pending: Vec<_> = self
            .inner
            .lock()
            .expect("accounts lock")
            .pending
            .drain()
            .map(|(_, pending)| pending)
            .collect();
        futures_util::future::join_all(pending.into_iter().map(finish_pending)).await;
    }
}

/// Stop an account bridge and its native client. The group is always a fresh
/// child's own, and it is also stopped on abort, when the `OwnedGroup` drops.
async fn close_group(group: &mut OwnedGroup, child: &mut tokio::process::Child) {
    let had_group = group.terminate();
    let _ = tokio::time::timeout(Duration::from_millis(700), child.wait()).await;
    if had_group {
        tokio::time::sleep(Duration::from_millis(100)).await;
        group.kill();
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    group.release();
}

struct Maintenance {
    manager: AccountManager,
    id: Uuid,
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        self.manager
            .inner
            .lock()
            .expect("accounts lock")
            .maintenance
            .remove(&self.id);
    }
}
async fn begin_maintenance(state: &AppState, id: Uuid) -> Result<Maintenance> {
    let _guard = state.operations.lock().await;
    let source = format!("account:{id}");
    let native_source = record(state, id)
        .ok()
        .and_then(|record| record.native_config.map(|reference| reference.source_id));
    for session in state
        .store
        .list_sessions(None)
        .map_err(ApiError::internal)?
    {
        let bound = session.native_source_id.as_deref() == Some(&source)
            || native_source
                .as_deref()
                .is_some_and(|source_id| session.native_source_id.as_deref() == Some(source_id))
            || session
                .endpoint_snapshot
                .as_ref()
                .and_then(|profile| profile.native_config.as_ref())
                .is_some_and(|reference| {
                    reference.source_id == source
                        || native_source.as_deref() == Some(reference.source_id.as_str())
                });
        if bound
            && (matches!(
                session.status,
                SessionStatus::Starting | SessionStatus::Running | SessionStatus::Waiting
            ) || state
                .runtime
                .get(&session.id.to_string())
                .is_some_and(|runtime| runtime.running())
                || state
                    .chats
                    .get(session.id)
                    .is_some_and(|runtime| runtime.running()))
        {
            return Err(ApiError::conflict(
                "Stop sessions using this account before changing its authentication or consuming a reset credit",
            ));
        }
    }
    state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .maintenance
        .insert(id);
    Ok(Maintenance {
        manager: state.accounts.clone(),
        id,
    })
}

async fn finish_pending(pending: PendingLogin) {
    let _ = pending.cancel.send(());
    let mut task = pending.task;
    if tokio::time::timeout(Duration::from_secs(45), &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountRecord {
    id: Uuid,
    name: String,
    provider: ProviderKind,
    profile_id: Uuid,
    /// Optional live reference to an existing native client home. The account
    /// directory still stores only AgentDock metadata; credentials stay in
    /// this referenced directory and are never copied.
    #[serde(default)]
    native_config: Option<NativeConfigReference>,
    /// Proxy for this account's own operations — sign-in and the usage check —
    /// which are separate from session traffic and were previously always direct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy_url: Option<String>,
}

/// Same rule as a session proxy, so one account setting can be carried to its
/// endpoint profile unchanged. SOCKS stays rejected because the native Claude
/// client does not support it, and credentials are rejected because this URL is
/// displayed in the account page.
fn validated_proxy(value: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if raw.len() > 400 || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ApiError::bad("Invalid proxy URL"));
    }
    crate::providers::validate_proxy(raw)?;
    Ok(Some(raw.to_owned()))
}
/// Node's `--use-env-proxy` reads the lowercase names; the child CLIs read the
/// uppercase ones. Both are set so one configuration covers fetch and clients.
fn proxy_environment(proxy: &str) -> Vec<(&'static str, String)> {
    [
        "http_proxy",
        "https_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "all_proxy",
    ]
    .into_iter()
    .map(|key| (key, proxy.to_owned()))
    .collect()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub login: bool,
    pub refresh_token: bool,
    pub quota: bool,
    pub reset_quota: bool,
    pub logout: bool,
    /// A user-triggered usage check exists without making AgentDock the OAuth
    /// owner. Claude usage is read from the official native credential context
    /// or the optional local status-line capture, never polled automatically.
    #[serde(default)]
    pub usage: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginView {
    pub url: String,
    pub id: Option<String>,
    pub user_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<f64>,
    pub resets_at: Option<serde_json::Value>,
    /// Friendly names are attached only when the native client reports an
    /// exact known duration. `primary`/`secondary` remain the source keys.
    pub window_kind: Option<WindowKind>,
    pub mapping_basis: Option<WindowMappingBasis>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    FiveHour,
    Weekly,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMappingBasis {
    DurationMinutes,
    /// The native client named the window itself (Claude status line payload).
    StatusLineWindow,
    /// The official Claude OAuth usage endpoint named the window.
    OAuthUsage,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Limits {
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
    pub reset_credits: Option<u64>,
    /// Provenance is kept separate from the count so an unavailable or
    /// malformed native response is never represented as zero credits.
    pub reset_credits_source: Option<ResetCreditsSource>,
}
/// Where a Claude usage figure came from. Queries are explicit and the value
/// remains a bounded display snapshot; it can be absent, stale, or older than
/// a reset.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageView {
    pub availability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Present when the provider asked us to wait before querying again. It is
    /// a hint for the page, never a timer AgentDock acts on by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    /// Set when a transient failure kept the figures from the previous check
    /// rather than blanking them. It stamps when those figures were read, so
    /// the page never presents stale numbers as current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_checked_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetCreditsSource {
    Official,
}
#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    pub id: Uuid,
    pub name: String,
    pub provider: ProviderKind,
    pub profile_id: Uuid,
    /// Source id is exposed for UI provenance only (never credential data).
    pub native_source_id: Option<String>,
    pub storage_path: String,
    /// True when this account points at a configuration the host already had,
    /// rather than one of its own. Its sign-in is then whatever that directory
    /// currently holds, so anything else editing it — another tool switching
    /// profiles, a native re-login — changes this account too. A user cannot
    /// infer that from a path, and being surprised by it is how an account
    /// stops working for no visible reason.
    pub shared_configuration: bool,
    pub status: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub checked_at: Option<String>,
    pub error: Option<String>,
    pub login: Option<LoginView>,
    pub limits: Option<Limits>,
    pub capabilities: Capabilities,
    pub guidance: Option<String>,
    pub reset_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageView>,
    /// Proxy used for this account's sign-in and usage check. Never carries
    /// credentials — those are rejected on the way in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}
#[derive(Deserialize)]
struct BridgeView {
    status: String,
    email: Option<String>,
    plan: Option<String>,
    checked_at: Option<String>,
    error: Option<String>,
    limits: Option<Limits>,
    capabilities: Capabilities,
    #[serde(default)]
    usage: Option<UsageView>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/accounts", get(list).post(create))
        .route("/api/accounts/import-native", post(import_native))
        .route("/api/accounts/{id}", get(read))
        .route("/api/accounts/{id}/rename", post(rename))
        .route("/api/accounts/{id}/remove", post(remove))
        .route("/api/accounts/{id}/proxy", post(set_proxy))
        .route("/api/accounts/{id}/refresh", post(refresh))
        .route("/api/accounts/{id}/usage", post(usage))
        .route("/api/accounts/{id}/login", post(login))
        .route("/api/accounts/{id}/cancel-login", post(cancel_login))
        .route("/api/accounts/{id}/logout", post(logout))
        .route("/api/accounts/{id}/reset-quota", post(reset_quota))
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}

fn protected_directory(path: &Path, create: bool) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(ApiError::bad(
                "Account storage must be a real directory, not a symlink",
            ));
        }
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(ApiError::internal)?;
        }
        Err(_) => return Err(ApiError::missing("Account storage")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(ApiError::internal)?;
    }
    dunce::canonicalize(path).map_err(ApiError::internal)
}
fn root_directory(state: &AppState, create: bool) -> Result<PathBuf> {
    let state_dir = protected_directory(&state.state_dir, false)?;
    protected_directory(&state_dir.join("accounts"), create)
}
fn directory(state: &AppState, id: Uuid) -> Result<PathBuf> {
    let root = root_directory(state, false)?;
    let directory = protected_directory(&root.join(id.to_string()), false)?;
    if !directory.starts_with(root) {
        return Err(ApiError::bad("Account path escaped its storage root"));
    }
    for file in [
        "account.json",
        "auth.json",
        "config.toml",
        ".credentials.json",
        "quota-reset-attempts.json",
        "usage.json",
    ] {
        if let Ok(metadata) = fs::symlink_metadata(directory.join(file))
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(ApiError::bad(
                "Account files must be regular files, not symlinks",
            ));
        }
    }
    protect_files(&directory)?;
    Ok(directory)
}
fn protect_files(directory: &Path) -> Result<()> {
    for name in [
        "account.json",
        "auth.json",
        "config.toml",
        ".credentials.json",
        "quota-reset-attempts.json",
        "usage.json",
    ] {
        let path = directory.join(name);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ApiError::bad("Account storage contains an unsafe file"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(ApiError::internal)?;
            }
        }
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(ApiError::internal)?;
    file.write_all(bytes).map_err(ApiError::internal)?;
    file.sync_all().map_err(ApiError::internal)
}

/// Replace existing account metadata atomically. Staging plus rename means a
/// crash mid-write leaves the previous record intact rather than a truncated one.
fn write_metadata_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(ApiError::bad("Account metadata must be a regular file"));
    }
    let staging = path.with_file_name(format!(".account-{}.tmp", Uuid::new_v4()));
    write_new(&staging, bytes)?;
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(ApiError::internal(error));
    }
    Ok(())
}

fn write_metadata_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let staging = path.with_file_name(format!(".account-{}.tmp", Uuid::new_v4()));
    write_new(&staging, bytes)?;
    if fs::symlink_metadata(path).is_ok() {
        let _ = fs::remove_file(&staging);
        return Err(ApiError::conflict("Account metadata already exists"));
    }
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(ApiError::internal(error));
    }
    Ok(())
}
fn record(state: &AppState, id: Uuid) -> Result<AccountRecord> {
    let directory = directory(state, id)?;
    let file = directory.join("account.json");
    if fs::metadata(&file).map_err(ApiError::internal)?.len() > 16 * 1024 {
        return Err(ApiError::bad("Account metadata exceeds limit"));
    }
    let record: AccountRecord =
        serde_json::from_slice(&fs::read(file).map_err(ApiError::internal)?)
            .map_err(|_| ApiError::bad("Invalid account metadata"))?;
    if record.id != id || record.provider == ProviderKind::Terminal {
        return Err(ApiError::bad("Account metadata identity mismatch"));
    }
    Ok(record)
}

fn reset_keys(state: &AppState, id: Uuid) -> Result<Vec<String>> {
    let path = directory(state, id)?.join("quota-reset-attempts.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ApiError::internal(error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        return Err(ApiError::bad("Invalid quota-reset retry metadata"));
    }
    let keys: Vec<String> = serde_json::from_slice(&fs::read(path).map_err(ApiError::internal)?)
        .map_err(|_| ApiError::bad("Invalid quota-reset retry metadata"))?;
    if keys.len() > 64
        || keys
            .iter()
            .any(|key| key.is_empty() || key.len() > 200 || key.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Invalid quota-reset retry metadata"));
    }
    Ok(keys)
}
fn persist_reset_key(state: &AppState, id: Uuid, key: &str) -> Result<()> {
    let mut keys = reset_keys(state, id)?;
    if keys.iter().any(|existing| existing == key) {
        return Ok(());
    }
    if keys.len() == 64 {
        keys.remove(0);
    }
    keys.push(key.into());
    let path = directory(state, id)?.join("quota-reset-attempts.json");
    let staging = path.with_file_name(format!(".reset-{}.tmp", Uuid::new_v4()));
    write_new(
        &staging,
        &serde_json::to_vec(&keys).map_err(ApiError::internal)?,
    )?;
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        let _ = fs::remove_file(staging);
        return Err(ApiError::bad("Invalid quota-reset retry metadata"));
    }
    if let Err(error) = fs::rename(&staging, &path) {
        let _ = fs::remove_file(staging);
        return Err(ApiError::internal(error));
    }
    #[cfg(unix)]
    {
        fs::File::open(path.parent().expect("account directory"))
            .and_then(|directory| directory.sync_all())
            .map_err(ApiError::internal)?;
    }
    Ok(())
}
pub fn pin(state: &AppState, source_id: &str) -> Result<(ProviderKind, NativeConfigReference)> {
    let id = source_id
        .strip_prefix("account:")
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| ApiError::bad("Invalid account source"))?;
    let record = record(state, id)?;
    if let Some(reference) = record.native_config {
        // Re-pin the host source on every use so an unavailable or changed
        // native directory cannot silently become an account configuration.
        let (provider, current) = crate::native_config::pin(state, &reference.source_id)?;
        if provider != record.provider || current != reference {
            return Err(ApiError::conflict(
                "Native account configuration identity changed",
            ));
        }
        return Ok((provider, current));
    }
    let config_dir = directory(state, id)?
        .to_str()
        .ok_or_else(|| ApiError::bad("Account directory must be UTF-8"))?
        .to_owned();
    Ok((
        record.provider,
        NativeConfigReference {
            source_id: format!("account:{id}"),
            config_env: Some(config_dir.clone()),
            config_dir,
        },
    ))
}
pub fn validate_reference(
    state: &AppState,
    provider: &ProviderKind,
    reference: &NativeConfigReference,
) -> Result<PathBuf> {
    let (actual_provider, actual) = pin(state, &reference.source_id)?;
    if actual_provider != *provider || actual != *reference {
        return Err(ApiError::conflict("Account configuration identity changed"));
    }
    Ok(PathBuf::from(actual.config_dir))
}
fn isolated_environment(
    provider: &ProviderKind,
    reference: &NativeConfigReference,
) -> (BTreeMap<String, String>, Vec<String>) {
    let key = if *provider == ProviderKind::Codex {
        "CODEX_HOME"
    } else {
        "CLAUDE_CONFIG_DIR"
    };
    let environment = reference
        .config_env
        .as_ref()
        .map(|config_env| [(key.into(), config_env.clone())].into())
        .unwrap_or_default();
    let remove = env::vars_os()
        .filter_map(|(key, _)| {
            let text = key.to_string_lossy().into_owned();
            let upper = text.to_ascii_uppercase();
            ([
                "AGENTDOCK_",
                "OPENAI_",
                "ANTHROPIC_",
                "CLAUDE_",
                "CODEX_",
                "AZURE_OPENAI_",
            ]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
                || upper == "CLAUDECODE")
                .then_some(text)
        })
        .filter(|existing| existing != key)
        .collect();
    (environment, remove)
}
pub fn build(
    state: &AppState,
    provider: &ProviderKind,
    reference: &NativeConfigReference,
    cwd: PathBuf,
) -> Result<SpawnSpec> {
    validate_reference(state, provider, reference)?;
    let id = Uuid::parse_str(reference.source_id.trim_start_matches("account:"))
        .map_err(|_| ApiError::bad("Invalid account source"))?;
    let inner = state.accounts.inner.lock().expect("accounts lock");
    if inner.pending.contains_key(&id) || inner.maintenance.contains(&id) {
        return Err(ApiError::conflict(
            "This account is being maintained; wait for its account operation to finish",
        ));
    }
    drop(inner);
    let mut spec = account_spec(&state.state_dir, provider, reference, cwd);
    if *provider == ProviderKind::ClaudeCode {
        // Let this session record its own usage payload locally as an opt-in
        // fallback. The account page still prefers the official OAuth usage
        // query when the user explicitly asks for it.
        let (overlay, capture) =
            crate::native_config::claude_statusline(state, reference.config_env.as_deref());
        spec.args.extend(overlay);
        spec.env.extend(capture);
        spec.env_remove.retain(|key| !spec.env.contains_key(key));
    }
    Ok(spec)
}
fn account_spec(
    state_dir: &Path,
    provider: &ProviderKind,
    reference: &NativeConfigReference,
    cwd: PathBuf,
) -> SpawnSpec {
    let (environment, remove) = isolated_environment(provider, reference);
    let program = crate::clients::program(state_dir, provider);
    SpawnSpec {
        program,
        args: vec![],
        cwd,
        env: environment,
        env_remove: remove,
    }
}

fn initial_view(state: &AppState, record: &AccountRecord) -> Result<AccountView> {
    let metadata_directory = directory(state, record.id)?;
    let directory = record
        .native_config
        .as_ref()
        .map(|reference| PathBuf::from(&reference.config_dir))
        .unwrap_or_else(|| metadata_directory.clone());
    let codex = record.provider == ProviderKind::Codex;
    let guidance = if codex {
        None
    } else {
        // Where this account's sign-in has to land.
        //
        // A linked account signs in wherever the configuration it references
        // already points, which for the host default is no override at all. An
        // account that owns its directory has to be told to sign in *there*:
        // without the override the command writes the credentials into the
        // host's own configuration, the account's directory stays empty, and
        // the account never works while appearing to exist.
        let login_home = match record.native_config.as_ref() {
            Some(reference) => reference.config_env.clone(),
            None => Some(metadata_directory.to_string_lossy().into_owned()),
        };
        let config_prefix = login_home
            .map(|value| format!(" CLAUDE_CONFIG_DIR='{}'", value.replace('\'', "'\\''")))
            .unwrap_or_default();
        // The command is the part a user has to act on; everything else this
        // used to say about token handling belongs in the README, not in a
        // panel read every time an account is opened.
        Some(format!(
            "env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN{} claude auth login",
            config_prefix
        ))
    };
    Ok(AccountView {
        id: record.id,
        name: record.name.clone(),
        provider: record.provider.clone(),
        profile_id: record.profile_id,
        native_source_id: record
            .native_config
            .as_ref()
            .map(|reference| reference.source_id.clone()),
        storage_path: directory.to_string_lossy().into_owned(),
        // An account AgentDock created owns its directory; one that references a
        // configuration the host already had does not.
        shared_configuration: record.native_config.is_some(),
        proxy_url: record.proxy_url.clone(),
        status: "unknown".into(),
        email: None,
        plan: None,
        checked_at: None,
        error: None,
        login: None,
        limits: None,
        capabilities: Capabilities {
            login: codex,
            refresh_token: codex,
            logout: codex,
            usage: !codex,
            ..Default::default()
        },
        guidance,
        reset_outcome: None,
        usage: None,
    })
}
/// A successful usage reading, kept in AgentDock's own account directory. The
/// provider allows very few checks, so re-reading this costs nothing and lets
/// the page show figures without spending an allowance. It is never written
/// into the native client's configuration directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedUsage {
    limits: Limits,
    /// When the figures were actually read, so they are never shown as live.
    checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}
fn usage_cache_path(state: &AppState, id: Uuid) -> Result<PathBuf> {
    Ok(directory(state, id)?.join("usage.json"))
}
fn read_usage_cache(state: &AppState, id: Uuid) -> Option<CachedUsage> {
    let path = usage_cache_path(state, id).ok()?;
    if fs::metadata(&path).ok()?.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn write_usage_cache(state: &AppState, id: Uuid, cached: &CachedUsage) {
    // A cache that cannot be written is not worth failing a good reading over.
    if let Ok(path) = usage_cache_path(state, id)
        && let Ok(bytes) = serde_json::to_vec(cached)
    {
        let _ = fs::write(path, bytes);
    }
}
fn view(state: &AppState, id: Uuid) -> Result<AccountView> {
    let record = record(state, id)?;
    let cached = state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .views
        .get(&id)
        .cloned();
    let mut view = cached.map_or_else(|| initial_view(state, &record), Ok)?;
    if let Some(profile) = state
        .store
        .get_endpoint_profile(record.profile_id)
        .map_err(ApiError::internal)?
    {
        view.name = profile.name;
    }
    // Figures the user already paid an allowance for survive a status refresh
    // and a server restart, but always arrive labelled with when they were read.
    if view
        .limits
        .as_ref()
        .is_none_or(|limits| limits.primary.is_none() && limits.secondary.is_none())
        && let Some(cached) = read_usage_cache(state, id)
    {
        view.limits = Some(cached.limits);
        let usage = view.usage.get_or_insert_with(|| UsageView {
            availability: "cached".into(),
            capture_path: None,
            captured_at: None,
            source: None,
            retry_after_seconds: None,
            retained_checked_at: None,
        });
        usage.source = cached.source;
        usage.retained_checked_at = Some(cached.checked_at);
    }
    Ok(view)
}
async fn list(State(state): State<AppState>) -> Result<Json<Vec<AccountView>>> {
    tokio::task::spawn_blocking(move || {
        let _guard = state
            .accounts
            .metadata
            .lock()
            .expect("account metadata lock");
        let root = match root_directory(&state, false) {
            Ok(path) => path,
            Err(error) if error.status == axum::http::StatusCode::NOT_FOUND => {
                return Ok(Json(Vec::new()));
            }
            Err(error) => return Err(error),
        };
        let mut result = Vec::new();
        for (index, entry) in fs::read_dir(root).map_err(ApiError::internal)?.enumerate() {
            if index >= 2000 {
                return Err(ApiError::bad("Too many account entries"));
            }
            let entry = entry.map_err(ApiError::internal)?;
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|id| Uuid::parse_str(id).ok())
            {
                result.push(view(&state, id)?);
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(Json(result))
    })
    .await
    .map_err(ApiError::internal)?
}
async fn read(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
) -> Result<Json<AccountView>> {
    tokio::task::spawn_blocking(move || view(&state, id).map(Json))
        .await
        .map_err(ApiError::internal)?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAccount {
    name: String,
    provider: ProviderKind,
}
async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateAccount>,
) -> Result<Json<AccountView>> {
    tokio::task::spawn_blocking(move || {
        if input.name.trim().is_empty()
            || input.name.trim().len() > 120
            || input.provider == ProviderKind::Terminal
        {
            return Err(ApiError::bad(
                "Choose Claude Code or Codex and a name of 1–120 bytes",
            ));
        }
        let _guard = state
            .accounts
            .metadata
            .lock()
            .expect("account metadata lock");
        let record = AccountRecord {
            id: Uuid::new_v4(),
            name: input.name.trim().into(),
            provider: input.provider,
            profile_id: Uuid::new_v4(),
            native_config: None,
            proxy_url: None,
        };
        let directory = protected_directory(
            &root_directory(&state, true)?.join(record.id.to_string()),
            true,
        )?;
        let metadata_path = directory.join("account.json");
        write_metadata_new(
            &metadata_path,
            &serde_json::to_vec(&record).map_err(ApiError::internal)?,
        )?;
        if record.provider == ProviderKind::Codex {
            write_new(
                &directory.join("config.toml"),
                b"cli_auth_credentials_store = \"file\"\n",
            )?;
        }
        let (_, reference) = pin(&state, &format!("account:{}", record.id))?;
        let profile = EndpointProfile {
            id: record.profile_id,
            name: record.name.clone(),
            provider: record.provider.clone(),
            endpoint_url: None,
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: Some(reference),
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        };
        if let Err(error) = state.store.create_endpoint_profile(&profile) {
            let _ = fs::remove_file(metadata_path);
            let _ = fs::remove_file(directory.join("config.toml"));
            let _ = fs::remove_dir(directory);
            return Err(ApiError::internal(error));
        }
        initial_view(&state, &record).map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportNativeAccount {
    source_id: String,
    name: Option<String>,
    #[serde(default)]
    confirmed_shared_config: bool,
}

/// Link an existing Claude/Codex native home as an official account. This is
/// deliberately a reference-only operation: no source files or credentials
/// are opened, copied, or returned to the browser.
async fn import_native(
    State(state): State<AppState>,
    Json(input): Json<ImportNativeAccount>,
) -> Result<Json<AccountView>> {
    if !input.confirmed_shared_config {
        return Err(ApiError::bad(
            "Confirm sharing the original client configuration before importing it",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let _guard = state
            .accounts
            .metadata
            .lock()
            .expect("account metadata lock");
        let config_state = state.clone();
        let (provider, reference) = crate::native_config::pin(&config_state, &input.source_id)?;
        if provider == ProviderKind::Terminal {
            return Err(ApiError::bad(
                "Terminal has no native account configuration",
            ));
        }
        // Import is idempotent per native source. Reusing an existing account
        // keeps one endpoint/profile for one host configuration instead of
        // creating duplicate account rows after a repeated click.
        if let Ok(root) = root_directory(&state, false) {
            for entry in fs::read_dir(root).map_err(ApiError::internal)? {
                let entry = entry.map_err(ApiError::internal)?;
                let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|value| Uuid::parse_str(value).ok())
                else {
                    continue;
                };
                if let Ok(existing) = record(&state, id)
                    && existing
                        .native_config
                        .as_ref()
                        .is_some_and(|current| current.source_id == reference.source_id)
                {
                    return initial_view(&state, &existing).map(Json);
                }
            }
        }
        let name = bounded_account_name(input.name.as_deref().unwrap_or(match provider {
            ProviderKind::Codex => "Imported Codex account",
            ProviderKind::ClaudeCode => "Imported Claude Code account",
            ProviderKind::Terminal => unreachable!(),
        }))?;
        let record = AccountRecord {
            id: Uuid::new_v4(),
            name: name.clone(),
            provider: provider.clone(),
            profile_id: Uuid::new_v4(),
            native_config: Some(reference.clone()),
            proxy_url: None,
        };
        let metadata_dir = protected_directory(
            &root_directory(&state, true)?.join(record.id.to_string()),
            true,
        )?;
        let metadata_path = metadata_dir.join("account.json");
        write_metadata_new(
            &metadata_path,
            &serde_json::to_vec(&record).map_err(ApiError::internal)?,
        )?;
        let profile = EndpointProfile {
            id: record.profile_id,
            name,
            provider: provider.clone(),
            endpoint_url: None,
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: Some(reference),
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        };
        if let Err(error) = state.store.create_endpoint_profile(&profile) {
            let _ = fs::remove_file(metadata_path);
            let _ = fs::remove_dir(metadata_dir);
            return Err(ApiError::internal(error));
        }
        initial_view(&state, &record).map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

fn bounded_account_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 || value.chars().any(char::is_control) {
        return Err(ApiError::bad("Choose an account name of 1–120 bytes"));
    }
    Ok(value.into())
}

/// Used by the profile API to keep account-linked templates managed from the
/// account screen, including imported references whose source id is not
/// `account:<uuid>`.
pub fn owns_profile(state: &AppState, profile_id: Uuid) -> bool {
    let root = match root_directory(state, false) {
        Ok(root) => root,
        Err(_) => return false,
    };
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .any(|id| {
            Uuid::parse_str(&id)
                .ok()
                .and_then(|id| record(state, id).ok())
                .is_some_and(|record| record.profile_id == profile_id)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdock_persistence::Store;
    use agentdock_runtime::RuntimeManager;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    struct Fixture {
        state: AppState,
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = env::temp_dir().join(format!("agentdock-account-test-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let root = dunce::canonicalize(root).unwrap();
            let state = AppState {
                store: Arc::new(Store::open(":memory:").unwrap()),
                runtime: RuntimeManager::new(),
                state_dir: root.clone(),
                browse_roots: vec![root.clone()],
                workspace_roots: vec![root.clone()],
                native_sources: vec![],
                native_bridge: root.join("unused-history.mjs"),
                chat_bridge: root.join("unused-chat.mjs"),
                chats: crate::conversations::ChatManager::default(),
                accounts: AccountManager::default(),
                security: crate::security::Security::for_test(
                    "127.0.0.1:8787".parse().unwrap(),
                    None,
                ),
                claude_manual_mode: true,
                operations: Arc::new(tokio::sync::Mutex::new(())),
            };
            Self { state, root }
        }
        async fn create(&self, provider: ProviderKind) -> AccountView {
            create(
                State(self.state.clone()),
                Json(CreateAccount {
                    name: "Fixture account".into(),
                    provider,
                }),
            )
            .await
            .unwrap()
            .0
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn accounts_create_only_protected_metadata_and_link_a_reusable_profile() {
        let f = Fixture::new();
        let account = f.create(ProviderKind::Codex).await;
        assert_eq!(account.status, "unknown");
        assert!(account.email.is_none());
        assert!(account.limits.is_none());
        let path = PathBuf::from(&account.storage_path);
        assert!(
            !path.join("auth.json").exists(),
            "Creating an account must not log in or copy credentials"
        );
        let metadata = fs::read_to_string(path.join("account.json")).unwrap();
        assert!(!metadata.contains("token"));
        let profile = f
            .state
            .store
            .get_endpoint_profile(account.profile_id)
            .unwrap()
            .unwrap();
        let reference = profile.native_config.unwrap();
        assert_eq!(reference.source_id, format!("account:{}", account.id));
        assert_eq!(
            reference.config_env.as_deref(),
            Some(account.storage_path.as_str())
        );
        assert_eq!(pin(&f.state, &reference.source_id).unwrap().1, reference);
        assert_eq!(list(State(f.state.clone())).await.unwrap().0.len(), 1);
        // No token read is needed to use/validate this reference, even if the
        // native client's credential format is opaque to AgentDock.
        write_new(
            &path.join("auth.json"),
            b"opaque fixture credentials - intentionally not JSON",
        )
        .unwrap();
        assert_eq!(
            read(State(f.state.clone()), RoutePath(account.id))
                .await
                .unwrap()
                .0
                .status,
            "unknown"
        );
        assert_eq!(
            validate_reference(&f.state, &ProviderKind::Codex, &reference).unwrap(),
            path
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(path.join("account.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.join("config.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn import_native_links_existing_home_without_copying_credentials() {
        let mut f = Fixture::new();
        let native = f.root.join("native-codex");
        fs::create_dir(&native).unwrap();
        f.state
            .native_sources
            .push(crate::native_history::NativeSource {
                id: "fixture-codex".into(),
                provider: ProviderKind::Codex,
                label: "Fixture Codex".into(),
                config_dir: native.clone(),
                config_env: None,
            });
        let account = import_native(
            State(f.state.clone()),
            Json(ImportNativeAccount {
                source_id: "fixture-codex".into(),
                name: Some("Imported fixture".into()),
                confirmed_shared_config: true,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(account.native_source_id.as_deref(), Some("fixture-codex"));
        // A linked account's sign-in lives in a directory the host owns, so
        // anything else editing it changes this account too. Saying so is the
        // difference between "my account stopped working for no reason" and
        // "the thing I share it with switched it".
        assert!(account.shared_configuration);
        assert_eq!(account.storage_path, native.to_string_lossy());
        assert!(
            !f.root
                .join("accounts")
                .join(account.id.to_string())
                .join("auth.json")
                .exists()
        );
        let profile = f
            .state
            .store
            .get_endpoint_profile(account.profile_id)
            .unwrap()
            .unwrap();
        assert_eq!(profile.native_config.unwrap().source_id, "fixture-codex");
        assert!(owns_profile(&f.state, account.profile_id));
        let duplicate = import_native(
            State(f.state.clone()),
            Json(ImportNativeAccount {
                source_id: "fixture-codex".into(),
                name: Some("Should not duplicate".into()),
                confirmed_shared_config: true,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(duplicate.id, account.id);
        assert_eq!(f.state.store.list_endpoint_profiles().unwrap().len(), 1);
        assert_eq!(
            import_native(
                State(f.state.clone()),
                Json(ImportNativeAccount {
                    source_id: "fixture-codex".into(),
                    name: None,
                    confirmed_shared_config: false
                })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn claude_has_native_guidance_without_managed_subscription_oauth_or_quota() {
        let f = Fixture::new();
        let account = f.create(ProviderKind::ClaudeCode).await;
        assert!(
            !account.capabilities.login
                && !account.capabilities.quota
                && !account.capabilities.reset_quota
                && !account.capabilities.refresh_token
        );
        let guidance = account.guidance.clone().unwrap();
        assert!(guidance.contains("claude auth login"));
        // An account AgentDock created owns its directory, so nothing else on
        // the host edits its sign-in.
        assert!(!account.shared_configuration);
        // And its sign-in has to land in that directory. Without the override
        // the command writes into the host configuration instead, leaving this
        // account empty while it still appears in the list.
        assert!(
            guidance.contains(&format!("CLAUDE_CONFIG_DIR='{}'", account.storage_path)),
            "an owned account signs in into its own directory: {guidance}"
        );
        assert_eq!(
            login(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Login {
                    mode: "device".into()
                })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            refresh(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Refresh {
                    refresh_token: true
                })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
        assert!(!f.state.accounts.pending(account.id));
    }
    #[tokio::test]
    async fn usage_is_an_explicit_claude_only_check_that_never_reports_quota_capability() {
        let f = Fixture::new();
        let claude = f.create(ProviderKind::ClaudeCode).await;
        // Usage exists as an on-demand check; it is not managed quota, so it
        // must not imply a reset credit or a quota interface.
        assert!(claude.capabilities.usage);
        assert!(!claude.capabilities.quota && !claude.capabilities.reset_quota);
        // Nothing is reported until the user asks, so a fresh account shows no
        // usage snapshot at all rather than an assumed zero.
        assert!(claude.usage.is_none() && claude.limits.is_none());
        let codex = f.create(ProviderKind::Codex).await;
        assert!(!codex.capabilities.usage);
        assert_eq!(
            usage(State(f.state.clone()), RoutePath(codex.id))
                .await
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );
        assert!(!f.state.accounts.pending(claude.id));
    }
    #[tokio::test]
    async fn a_failed_usage_query_keeps_previous_figures_without_calling_them_current() {
        let f = Fixture::new();
        let base = f.create(ProviderKind::ClaudeCode).await;
        let window = |percent: f64| LimitWindow {
            used_percent: percent,
            window_minutes: Some(300.0),
            window_kind: Some(WindowKind::FiveHour),
            mapping_basis: Some(WindowMappingBasis::OAuthUsage),
            resets_at: None,
        };
        let succeeded = AccountView {
            checked_at: Some("2026-09-15T02:00:00Z".into()),
            limits: Some(Limits {
                primary: Some(window(36.0)),
                ..Default::default()
            }),
            usage: Some(UsageView {
                availability: "available".into(),
                source: Some("claude_oauth_usage".into()),
                ..empty_usage()
            }),
            ..base.clone()
        };
        let failed = || AccountView {
            checked_at: Some("2026-09-15T02:05:00Z".into()),
            limits: None,
            usage: Some(UsageView {
                availability: "rate_limited".into(),
                retry_after_seconds: Some(30),
                ..empty_usage()
            }),
            ..base.clone()
        };

        let mut current = failed();
        retain_previous_usage(&mut current, Some(succeeded.clone()));
        let usage = current.usage.as_ref().unwrap();
        // The figures survive, but they are stamped as the earlier reading and
        // the attempt still reports why it failed.
        assert_eq!(current.limits.unwrap().primary.unwrap().used_percent, 36.0);
        assert_eq!(usage.availability, "rate_limited");
        assert_eq!(usage.retry_after_seconds, Some(30));
        assert_eq!(
            usage.retained_checked_at.as_deref(),
            Some("2026-09-15T02:00:00Z")
        );
        // Provenance follows the figures, never the failed attempt.
        assert_eq!(usage.source.as_deref(), Some("claude_oauth_usage"));

        // With nothing worth keeping, the failure stands on its own rather than
        // inventing a retained reading.
        let mut current = failed();
        retain_previous_usage(&mut current, Some(base.clone()));
        assert!(current.limits.is_none());
        assert!(current.usage.unwrap().retained_checked_at.is_none());

        // A successful query always wins over the previous figures.
        let mut current = AccountView {
            limits: Some(Limits {
                primary: Some(window(12.0)),
                ..Default::default()
            }),
            usage: Some(UsageView {
                availability: "available".into(),
                ..empty_usage()
            }),
            ..base.clone()
        };
        retain_previous_usage(&mut current, Some(succeeded));
        assert_eq!(current.limits.unwrap().primary.unwrap().used_percent, 12.0);
        assert!(current.usage.unwrap().retained_checked_at.is_none());
    }
    #[tokio::test]
    async fn a_paid_for_usage_reading_survives_refreshes_and_restarts() {
        let f = Fixture::new();
        let account = f.create(ProviderKind::ClaudeCode).await;
        let cached = CachedUsage {
            limits: Limits {
                primary: Some(LimitWindow {
                    used_percent: 30.0,
                    window_minutes: Some(300.0),
                    window_kind: Some(WindowKind::FiveHour),
                    mapping_basis: Some(WindowMappingBasis::OAuthUsage),
                    resets_at: None,
                }),
                ..Default::default()
            },
            checked_at: "2026-09-15T03:58:00Z".into(),
            source: Some("claude_oauth_usage".into()),
        };
        write_usage_cache(&f.state, account.id, &cached);

        // A fresh view — the state a restart or a plain status refresh leaves
        // behind — still shows the figures instead of an empty panel.
        let restored = view(&f.state, account.id).unwrap();
        let limits = restored.limits.as_ref().unwrap();
        assert_eq!(limits.primary.as_ref().unwrap().used_percent, 30.0);
        let usage = restored.usage.as_ref().unwrap();
        // They are labelled with when they were read, never presented as live.
        assert_eq!(
            usage.retained_checked_at.as_deref(),
            Some("2026-09-15T03:58:00Z")
        );
        assert_eq!(usage.source.as_deref(), Some("claude_oauth_usage"));
        assert_eq!(usage.availability, "cached");

        // The cache lives in AgentDock's own directory, never the native client's.
        let path = usage_cache_path(&f.state, account.id).unwrap();
        assert!(path.starts_with(&f.root) && path.ends_with("usage.json"));

        // A corrupt or oversized cache is ignored rather than failing the page.
        fs::write(&path, b"{not json").unwrap();
        assert!(read_usage_cache(&f.state, account.id).is_none());
        assert!(view(&f.state, account.id).is_ok());
    }
    #[tokio::test]
    async fn an_account_proxy_applies_to_its_own_operations_and_never_carries_credentials() {
        let f = Fixture::new();
        let account = f.create(ProviderKind::ClaudeCode).await;
        let patch = |value: Option<&str>| {
            let body = ProxyPatch {
                proxy_url: value.map(str::to_owned),
            };
            set_proxy(State(f.state.clone()), RoutePath(account.id), Json(body))
        };

        assert_eq!(
            patch(Some("http://127.0.0.1:7890"))
                .await
                .unwrap()
                .0
                .proxy_url
                .as_deref(),
            Some("http://127.0.0.1:7890")
        );
        // It is durable, so a sign-in after a restart still uses it.
        assert_eq!(
            record(&f.state, account.id).unwrap().proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            view(&f.state, account.id).unwrap().proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            patch(Some("https://proxy.internal:8443"))
                .await
                .unwrap()
                .0
                .proxy_url
                .as_deref(),
            Some("https://proxy.internal:8443")
        );

        // Credentials in the URL are refused: the account page displays it.
        for rejected in [
            "http://user:secret@127.0.0.1:7890",
            "http://user@127.0.0.1:7890",
            "ftp://127.0.0.1:21",
            // SOCKS is refused because the native Claude client cannot use it;
            // offering it would be an option that silently does nothing.
            "socks5h://10.0.0.1:1080",
            "not a url",
            "http://",
        ] {
            assert_eq!(
                patch(Some(rejected)).await.unwrap_err().status,
                StatusCode::BAD_REQUEST,
                "{rejected} must be refused"
            );
        }
        // A refused value leaves the previous one in place.
        assert_eq!(
            record(&f.state, account.id).unwrap().proxy_url.as_deref(),
            Some("https://proxy.internal:8443")
        );

        // Clearing returns the account to a direct connection.
        for cleared in [None, Some("   ")] {
            assert_eq!(patch(cleared).await.unwrap().0.proxy_url, None);
        }

        // Both cases are exported: Node's --use-env-proxy reads the lowercase
        // names while the native CLIs read the uppercase ones.
        let exported = proxy_environment("http://127.0.0.1:7890");
        let keys: Vec<_> = exported.iter().map(|(key, _)| *key).collect();
        assert!(keys.contains(&"https_proxy") && keys.contains(&"HTTPS_PROXY"));
        assert!(
            exported
                .iter()
                .all(|(_, value)| value == "http://127.0.0.1:7890")
        );
    }
    #[tokio::test]
    async fn accounts_can_be_renamed_and_removed_without_touching_a_linked_native_home() {
        let mut f = Fixture::new();
        let account = f.create(ProviderKind::ClaudeCode).await;
        let storage = PathBuf::from(&account.storage_path);

        // The name is shared with the endpoint profile, so the two cannot drift.
        let renamed = rename(
            State(f.state.clone()),
            RoutePath(account.id),
            Json(RenameAccount {
                name: "  Work Claude  ".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(renamed.name, "Work Claude");
        assert_eq!(record(&f.state, account.id).unwrap().name, "Work Claude");
        assert_eq!(
            f.state
                .store
                .get_endpoint_profile(account.profile_id)
                .unwrap()
                .unwrap()
                .name,
            "Work Claude"
        );
        for bad in ["", "   ", "bad\nname"] {
            assert_eq!(
                rename(
                    State(f.state.clone()),
                    RoutePath(account.id),
                    Json(RenameAccount { name: bad.into() })
                )
                .await
                .unwrap_err()
                .status,
                StatusCode::BAD_REQUEST
            );
        }

        // Removal is deliberate, never implicit.
        assert_eq!(
            remove(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Confirm { confirmed: false })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
        assert!(storage.exists());
        remove(
            State(f.state.clone()),
            RoutePath(account.id),
            Json(Confirm { confirmed: true }),
        )
        .await
        .unwrap();
        assert!(!storage.exists());
        assert!(
            f.state
                .store
                .get_endpoint_profile(account.profile_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            record(&f.state, account.id).unwrap_err().status,
            StatusCode::NOT_FOUND
        );

        // A linked account owns only its metadata: the referenced native home
        // and everything the user keeps there must survive removal untouched.
        let config = f.root.join("native-codex-home");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("auth.json"), "user credential").unwrap();
        f.state
            .native_sources
            .push(crate::native_history::NativeSource {
                id: "fixture-codex".into(),
                provider: ProviderKind::Codex,
                label: "Fixture Codex".into(),
                config_dir: config.clone(),
                config_env: None,
            });
        let linked = import_native(
            State(f.state.clone()),
            Json(ImportNativeAccount {
                source_id: "fixture-codex".into(),
                name: None,
                confirmed_shared_config: true,
            }),
        )
        .await
        .unwrap()
        .0;
        remove(
            State(f.state.clone()),
            RoutePath(linked.id),
            Json(Confirm { confirmed: true }),
        )
        .await
        .unwrap();
        assert!(
            config.join("auth.json").exists(),
            "the native home must be left alone"
        );
        assert_eq!(
            fs::read_to_string(config.join("auth.json")).unwrap(),
            "user credential"
        );
    }
    fn empty_usage() -> UsageView {
        UsageView {
            availability: String::new(),
            capture_path: None,
            captured_at: None,
            source: None,
            retry_after_seconds: None,
            retained_checked_at: None,
        }
    }
    #[tokio::test]
    async fn account_maintenance_blocks_bound_sessions_and_requires_explicit_consent() {
        let f = Fixture::new();
        let account = f.create(ProviderKind::Codex).await;
        let workspace = f
            .state
            .store
            .create_workspace("fixture", f.root.to_str().unwrap())
            .unwrap();
        let session = f
            .state
            .store
            .create_session_with_profile(
                workspace.id,
                ProviderKind::Codex,
                "fixture",
                Some(account.profile_id),
            )
            .unwrap();
        f.state
            .store
            .set_session_status(session.id, SessionStatus::Running)
            .unwrap();
        assert_eq!(
            logout(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Confirm { confirmed: true })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            login(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Login {
                    mode: "device".into()
                })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            logout(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(Confirm { confirmed: false })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            reset_quota(
                State(f.state.clone()),
                RoutePath(account.id),
                Json(ResetQuota {
                    confirmed: false,
                    idempotency_key: "fixture".into(),
                    credit_id: None
                })
            )
            .await
            .unwrap_err()
            .status,
            StatusCode::BAD_REQUEST
        );
        f.state
            .store
            .set_session_status(session.id, SessionStatus::Stopped)
            .unwrap();
        let guard = begin_maintenance(&f.state, account.id).await.unwrap();
        let reference = pin(&f.state, &format!("account:{}", account.id)).unwrap().1;
        assert_eq!(
            build(&f.state, &ProviderKind::Codex, &reference, f.root.clone())
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        drop(guard);
        assert!(
            !f.state
                .accounts
                .inner
                .lock()
                .unwrap()
                .maintenance
                .contains(&account.id)
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn account_metadata_and_credentials_symlinks_are_rejected_without_reading_targets() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let account = f.create(ProviderKind::Codex).await;
        let outside = f.root.join("outside-fixture");
        write_new(&outside, b"unchanged fixture").unwrap();
        let auth = PathBuf::from(&account.storage_path).join("auth.json");
        symlink(&outside, &auth).unwrap();
        assert_eq!(
            pin(&f.state, &format!("account:{}", account.id))
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );
        fs::remove_file(auth).unwrap();
        let metadata = PathBuf::from(&account.storage_path).join("account.json");
        fs::remove_file(&metadata).unwrap();
        symlink(&outside, &metadata).unwrap();
        assert_eq!(
            record(&f.state, account.id).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(fs::read(outside).unwrap(), b"unchanged fixture");
    }
    #[tokio::test]
    async fn account_routes_keep_management_authentication() {
        let mut f = Fixture::new();
        f.state.security = crate::security::Security::for_test(
            "127.0.0.1:8787".parse().unwrap(),
            Some("fixture-token-long-enough"),
        );
        let request = Request::builder()
            .uri("/api/accounts")
            .header("host", "127.0.0.1:8787")
            .body(Body::empty())
            .unwrap();
        let response = crate::router(f.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let request = Request::builder()
            .uri("/api/accounts")
            .header("host", "127.0.0.1:8787")
            .header("authorization", "Bearer fixture-token-long-enough")
            .body(Body::empty())
            .unwrap();
        let response = crate::router(f.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(to_bytes(response.into_body(), 4096).await.unwrap(), "[]");
    }

    #[tokio::test]
    async fn reset_retry_authorization_is_private_bounded_and_survives_manager_restart() {
        let mut f = Fixture::new();
        let account = f.create(ProviderKind::Codex).await;
        let generation = Uuid::new_v4();
        f.state
            .accounts
            .inner
            .lock()
            .unwrap()
            .generations
            .insert(account.id, generation);
        let metadata = record(&f.state, account.id).unwrap();
        apply_event(
            &f.state,
            &metadata,
            generation,
            json!({"type":"reset_authorized","idempotency_key":"original-retry-key"}),
        )
        .unwrap();
        f.state.accounts = AccountManager::default();
        assert_eq!(
            reset_keys(&f.state, account.id).unwrap(),
            vec!["original-retry-key"]
        );
        assert!(
            apply_event(
                &f.state,
                &metadata,
                generation,
                json!({"type":"reset_authorized","idempotency_key":"stale-attempt"})
            )
            .is_err()
        );
        for index in 0..66 {
            persist_reset_key(&f.state, account.id, &format!("attempt-{index}")).unwrap();
        }
        let keys = reset_keys(&f.state, account.id).unwrap();
        assert_eq!(keys.len(), 64);
        assert_eq!(keys[0], "attempt-2");
        assert!(!Path::new(&account.storage_path).join("auth.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = Path::new(&account.storage_path).join("quota-reset-attempts.json");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(f.root.join("outside-retry-file"), path).unwrap();
            assert!(reset_keys(&f.state, account.id).is_err());
        }
    }

    #[cfg(unix)]
    fn fixture_process_live(pid: u32) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&output.stdout);
        !status.trim().is_empty() && !status.trim().starts_with('Z')
    }
    #[cfg(unix)]
    async fn process_group_fixture() -> (tokio::process::Child, u32) {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; sleep 60 & echo $!; wait"])
            .process_group(0)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap()).lines();
        let descendant = tokio::time::timeout(Duration::from_secs(3), reader.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .parse()
            .unwrap();
        (child, descendant)
    }
    #[cfg(unix)]
    async fn assert_fixture_exited(pid: u32) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture_process_live(pid) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("owned fixture descendant must not remain running");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn account_group_cleanup_kills_term_ignoring_descendants() {
        let (mut child, descendant) = process_group_fixture().await;
        let mut group = OwnedGroup::new(child.id());
        close_group(&mut group, &mut child).await;
        assert_fixture_exited(descendant).await;
        for pid in [None, Some(0), Some(1)] {
            assert!(OwnedGroup::new(pid).pid().is_none());
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_account_actor_also_drops_its_owned_native_process_group() {
        let (child, descendant) = process_group_fixture().await;
        let (ready, sent) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _child = child;
            let _group = OwnedGroup::new(_child.id());
            let _ = ready.send(());
            std::future::pending::<()>().await;
        });
        sent.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_fixture_exited(descendant).await;
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Refresh {
    #[serde(default)]
    refresh_token: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    mode: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    confirmed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetQuota {
    confirmed: bool,
    idempotency_key: String,
    credit_id: Option<String>,
}

async fn refresh(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<Refresh>,
) -> Result<Json<AccountView>> {
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    if record.provider == ProviderKind::ClaudeCode && input.refresh_token {
        return Err(ApiError::bad(
            "Claude token refresh is managed by its official CLI; this operation is unsupported",
        ));
    }
    ensure_not_pending(&state, id)?;
    let _maintenance = if input.refresh_token {
        Some(begin_maintenance(&state, id).await?)
    } else {
        None
    };
    run_operation(
        &state,
        record,
        json!({"action":"read","refresh_token":input.refresh_token}),
    )
    .await?;
    Ok(Json(view(&state, id)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameAccount {
    name: String,
}
/// Rename the account and the endpoint profile that represents it, so the two
/// never drift apart. Nothing about its sign-in or native reference changes.
async fn rename(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<RenameAccount>,
) -> Result<Json<AccountView>> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Invalid account name"));
    }
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let mut record = record(&state, id)?;
    record.name = name.to_owned();
    let path = directory(&state, id)?.join("account.json");
    {
        let _metadata = state
            .accounts
            .metadata
            .lock()
            .expect("accounts metadata lock");
        write_metadata_replace(
            &path,
            &serde_json::to_vec(&record).map_err(ApiError::internal)?,
        )?;
    }
    if let Some(mut profile) = state
        .store
        .get_endpoint_profile(record.profile_id)
        .map_err(ApiError::internal)?
    {
        profile.name = name.to_owned();
        state
            .store
            .update_endpoint_profile(&profile)
            .map_err(ApiError::internal)?;
    }
    state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .views
        .entry(id)
        .and_modify(|view| view.name = name.to_owned());
    view(&state, id).map(Json)
}

/// Remove an account AgentDock created or linked. Only AgentDock's own account
/// directory is deleted; a linked native home is left completely untouched.
async fn remove(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<Confirm>,
) -> Result<axum::http::StatusCode> {
    if !input.confirmed {
        return Err(ApiError::bad(
            "Removing an account requires explicit confirmation",
        ));
    }
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    // Reuses the running-session check, so an account still in use cannot be
    // pulled out from under a live client.
    let _maintenance = begin_maintenance(&state, id).await?;
    // A sign-in still in flight for this account must not outlive it.
    let pending = state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .pending
        .remove(&id);
    if let Some(pending) = pending {
        finish_pending(pending).await;
    }
    let directory = directory(&state, id)?;
    // Sessions keep their immutable endpoint snapshot, so removing the template
    // does not rewrite any history.
    state
        .store
        .delete_endpoint_profile(record.profile_id)
        .map_err(ApiError::internal)?;
    // Always AgentDock's own account directory, never the referenced native
    // home: an isolated account's sign-in lives here and goes with it, while a
    // linked account only ever stored metadata beside the untouched original.
    fs::remove_dir_all(&directory).map_err(ApiError::internal)?;
    state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .views
        .remove(&id);
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyPatch {
    /// Null or empty clears the proxy, returning this account to a direct connection.
    proxy_url: Option<String>,
}
/// Set the proxy used for this account's own operations. It takes effect on the
/// next sign-in or usage check; nothing in flight is re-routed.
async fn set_proxy(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<ProxyPatch>,
) -> Result<Json<AccountView>> {
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    ensure_not_pending(&state, id)?;
    let proxy = validated_proxy(input.proxy_url.as_deref())?;
    let mut record = record(&state, id)?;
    record.proxy_url = proxy.clone();
    let path = directory(&state, id)?.join("account.json");
    let _metadata = state
        .accounts
        .metadata
        .lock()
        .expect("accounts metadata lock");
    write_metadata_replace(
        &path,
        &serde_json::to_vec(&record).map_err(ApiError::internal)?,
    )?;
    // Carry it to the account's endpoint profile so sessions started from this
    // account take the same route, instead of the proxy only covering sign-in.
    if let Some(mut profile) = state
        .store
        .get_endpoint_profile(record.profile_id)
        .map_err(ApiError::internal)?
        && profile.proxy_url != proxy
    {
        profile.proxy_url = proxy.clone();
        state
            .store
            .update_endpoint_profile(&profile)
            .map_err(ApiError::internal)?;
    }
    let mut view = view(&state, id)?;
    view.proxy_url = proxy;
    Ok(Json(view))
}
async fn usage(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
) -> Result<Json<AccountView>> {
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    if record.provider == ProviderKind::Codex {
        return Err(ApiError::bad(
            "Codex usage comes from the official app-server; refresh the account instead",
        ));
    }
    ensure_not_pending(&state, id)?;
    let previous = view(&state, id).ok();
    // An explicit request only. Nothing here polls, caches or infers usage, and
    // no credential or private endpoint is touched.
    run_operation(&state, record, json!({"action":"usage"})).await?;
    let mut current = view(&state, id)?;
    // Distinguish "capture is turned off" from "nothing captured yet" so the
    // page can say which one it is instead of asking for a session that would
    // never record anything.
    if !crate::native_config::usage_capture_enabled()
        && current
            .usage
            .as_ref()
            .is_some_and(|usage| usage.availability == "capture_missing")
        && let Some(usage) = current.usage.as_mut()
    {
        usage.availability = "capture_disabled".into();
    }
    // A reading the user spent an allowance on is persisted before anything
    // else can overwrite it, so it survives refreshes and restarts.
    if current
        .usage
        .as_ref()
        .is_some_and(|usage| usage.availability == "available")
        && let Some(limits) = current.limits.clone()
        && (limits.primary.is_some() || limits.secondary.is_some())
    {
        write_usage_cache(
            &state,
            id,
            &CachedUsage {
                limits,
                checked_at: current
                    .checked_at
                    .clone()
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                source: current
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.source.clone()),
            },
        );
    }
    retain_previous_usage(&mut current, previous.or_else(|| cached_view(&state, id)));
    Ok(Json(current))
}
/// The stored reading as a stand-in previous view, so a refused query can fall
/// back to it even when this process has never served a successful one.
fn cached_view(state: &AppState, id: Uuid) -> Option<AccountView> {
    let cached = read_usage_cache(state, id)?;
    let mut view = view(state, id).ok()?;
    view.checked_at = Some(cached.checked_at);
    view.limits = Some(cached.limits);
    view.usage = Some(UsageView {
        availability: "cached".into(),
        capture_path: None,
        captured_at: None,
        source: cached.source,
        retry_after_seconds: None,
        retained_checked_at: None,
    });
    Some(view)
}
/// The official endpoint rate-limits tightly, so pressing Check usage twice
/// would otherwise wipe figures the user just fetched. On a failed attempt keep
/// the previous ones, stamped with when they were read, rather than blanking
/// the panel or presenting them as current.
fn retain_previous_usage(current: &mut AccountView, previous: Option<AccountView>) {
    let has_windows = |limits: &Option<Limits>| {
        limits
            .as_ref()
            .is_some_and(|value| value.primary.is_some() || value.secondary.is_some())
    };
    if !current.usage.as_ref().is_some_and(|usage| {
        matches!(
            usage.availability.as_str(),
            "rate_limited" | "query_failed" | "unauthorized"
        )
    }) || has_windows(&current.limits)
    {
        return;
    }
    let Some(previous) = previous.filter(|previous| has_windows(&previous.limits)) else {
        return;
    };
    current.limits = previous.limits;
    if let Some(usage) = current.usage.as_mut() {
        // Provenance travels with the figures, not with the failed attempt, so
        // retained numbers are never relabelled as a different source.
        let before = previous.usage.as_ref();
        usage.source = before.and_then(|usage| usage.source.clone());
        usage.captured_at = before.and_then(|usage| usage.captured_at);
        usage.retained_checked_at = previous.checked_at;
    }
}
async fn logout(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<Confirm>,
) -> Result<Json<AccountView>> {
    if !input.confirmed {
        return Err(ApiError::bad("Logout requires explicit confirmation"));
    }
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    ensure_codex(&record)?;
    ensure_not_pending(&state, id)?;
    let _maintenance = begin_maintenance(&state, id).await?;
    run_operation(&state, record, json!({"action":"logout","confirmed":true})).await?;
    Ok(Json(view(&state, id)?))
}
async fn reset_quota(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<ResetQuota>,
) -> Result<Json<AccountView>> {
    if !input.confirmed
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.len() > 200
        || input.idempotency_key.chars().any(char::is_control)
        || input.credit_id.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 300 || value.chars().any(char::is_control)
        })
    {
        return Err(ApiError::bad(
            "Quota reset requires confirmation and a valid idempotency key",
        ));
    }
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    ensure_codex(&record)?;
    ensure_not_pending(&state, id)?;
    let _maintenance = begin_maintenance(&state, id).await?;
    let retry_authorized = reset_keys(&state, id)?.contains(&input.idempotency_key);
    // The bridge re-reads official credits immediately before consumption; a
    // cached UI number can never authorize a locally manufactured reset.
    run_operation(&state, record, json!({"action":"reset_quota","confirmed":true,"idempotency_key":input.idempotency_key,"credit_id":input.credit_id,"retry_authorized":retry_authorized})).await?;
    Ok(Json(view(&state, id)?))
}
fn ensure_codex(record: &AccountRecord) -> Result<()> {
    if record.provider != ProviderKind::Codex {
        Err(ApiError::bad(
            "This managed account operation is unsupported for Claude; use its official CLI directly",
        ))
    } else {
        Ok(())
    }
}
fn ensure_not_pending(state: &AppState, id: Uuid) -> Result<()> {
    if state.accounts.pending(id) {
        Err(ApiError::conflict(
            "An account login is already pending; cancel it before another operation",
        ))
    } else {
        Ok(())
    }
}

async fn login(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
    Json(input): Json<Login>,
) -> Result<Json<AccountView>> {
    if !matches!(input.mode.as_str(), "device" | "browser") {
        return Err(ApiError::bad("Choose device or browser login"));
    }
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    let record = record(&state, id)?;
    ensure_codex(&record)?;
    ensure_not_pending(&state, id)?;
    let _maintenance = begin_maintenance(&state, id).await?;
    if state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .pending
        .len()
        >= 4
    {
        return Err(ApiError::conflict("Too many pending account logins"));
    }
    let generation = Uuid::new_v4();
    let mut initial = view(&state, id)?;
    initial.status = "login_pending".into();
    initial.login = None;
    initial.error = None;
    {
        let mut inner = state.accounts.inner.lock().expect("accounts lock");
        inner.generations.insert(id, generation);
        inner.views.insert(id, initial);
    }
    let (cancel, receiver) = oneshot::channel();
    let (ready, first) = oneshot::channel();
    let (start, started) = oneshot::channel();
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        if started.await.is_err() {
            return;
        }
        let _ = bridge(
            &task_state,
            record,
            generation,
            json!({"action":"login","mode":input.mode}),
            Some(receiver),
            Some(ready),
        )
        .await;
        let mut inner = task_state.accounts.inner.lock().expect("accounts lock");
        if inner
            .pending
            .get(&id)
            .is_some_and(|pending| pending.generation == generation)
        {
            inner.pending.remove(&id);
        }
    });
    {
        let mut inner = state.accounts.inner.lock().expect("accounts lock");
        if inner.pending.len() >= 4 {
            task.abort();
            if let Some(view) = inner.views.get_mut(&id) {
                view.status = "unknown".into();
                view.error = Some("Too many pending account logins".into());
            }
            return Err(ApiError::conflict("Too many pending account logins"));
        }
        inner.pending.insert(
            id,
            PendingLogin {
                generation,
                cancel,
                task,
            },
        );
    }
    let _ = start.send(());
    let _ = tokio::time::timeout(Duration::from_secs(25), first).await;
    Ok(Json(view(&state, id)?))
}
async fn cancel_login(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<Uuid>,
) -> Result<Json<AccountView>> {
    let operation = state.accounts.operation(id);
    let _guard = operation.lock().await;
    record(&state, id)?;
    let _maintenance = begin_maintenance(&state, id).await?;
    let pending = state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .pending
        .remove(&id);
    if let Some(pending) = pending {
        finish_pending(pending).await;
    }
    let mut current = view(&state, id)?;
    if current.status == "login_pending" {
        current.status = "unknown".into();
        current.login = None;
        current.error = Some("Login cancelled; refresh to check the official account state".into());
        state
            .accounts
            .inner
            .lock()
            .expect("accounts lock")
            .views
            .insert(id, current.clone());
    }
    Ok(Json(current))
}

async fn run_operation(state: &AppState, record: AccountRecord, job: Value) -> Result<()> {
    if let Ok(mut view) = view(state, record.id) {
        view.reset_outcome = None;
        state
            .accounts
            .inner
            .lock()
            .expect("accounts lock")
            .views
            .insert(record.id, view);
    }
    let generation = Uuid::new_v4();
    state
        .accounts
        .inner
        .lock()
        .expect("accounts lock")
        .generations
        .insert(record.id, generation);
    bridge(state, record, generation, job, None, None).await
}
async fn bridge(
    state: &AppState,
    record: AccountRecord,
    generation: Uuid,
    mut job: Value,
    cancel: Option<oneshot::Receiver<()>>,
    mut ready: Option<oneshot::Sender<()>>,
) -> Result<()> {
    let id = record.id;
    let result = async {
        let (_, reference) = pin(state, &format!("account:{id}"))?;
        let spec = account_spec(
            &state.state_dir,
            &record.provider,
            &reference,
            reference.config_dir.clone().into(),
        );
        job["provider"] = serde_json::to_value(&record.provider).map_err(ApiError::internal)?;
        job["config_dir"] = json!(reference.config_dir);
        job["config_env"] = json!(reference.config_env);
        job["program"] = json!(spec.program);
        let script = crate::bridge::script(&state.state_dir, "AGENTDOCK_ACCOUNT_BRIDGE", "account.mjs");
        // --use-env-proxy makes the bridge's own fetch (the usage query) honour
        // the proxy; the same variables reach the native CLIs it spawns.
        let node_options: &[&str] = if record.proxy_url.is_some() { &["--use-env-proxy"] } else { &[] };
        let mut command = crate::bridge::node_command(node_options, script);
        command.current_dir(&reference.config_dir);
        for key in spec.env_remove { command.env_remove(key); } command.envs(spec.env);
        if let Some(proxy) = record.proxy_url.as_deref() {
            command.envs(proxy_environment(proxy));
        } else {
            // An account with no proxy configured stays direct rather than
            // silently inheriting whatever the host shell happened to export.
            for (key, _) in proxy_environment("") { command.env_remove(key); }
        }
        let mut child = command.spawn().map_err(|_| ApiError::bad("Account management needs Node.js and the installed official native client bridge"))?;
        let mut group = OwnedGroup::new(child.id());
        let mut input = child.stdin.take().ok_or_else(|| ApiError::bad("Account bridge input unavailable"))?;
        let io_result = async {
            tokio::time::timeout(Duration::from_secs(3), input.write_all(format!("{}\n",job).as_bytes())).await.map_err(|_|ApiError::bad("Account bridge input timed out"))?.map_err(ApiError::internal)?;
            let output = child.stdout.take().ok_or_else(|| ApiError::bad("Account bridge output unavailable"))?;
            let mut lines = BufReader::new(output.take(1024 * 1024)).lines();
            let mut cancel = cancel;
            let mut deadline = tokio::time::Instant::now() + Duration::from_secs(if job["action"] == "login" {31*60} else {60});
            let mut received_view = false;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => return Err(ApiError::bad("Official account operation timed out")),
                    _ = async {if let Some(receiver)=cancel.as_mut(){let _=receiver.await;}else{std::future::pending::<()>().await;}} => {
                        cancel=None;
                        tokio::time::timeout(Duration::from_secs(2),input.write_all(b"{\"type\":\"cancel\"}\n")).await.map_err(|_|ApiError::bad("Login cancellation input timed out"))?.map_err(ApiError::internal)?;
                        deadline=tokio::time::Instant::now()+Duration::from_secs(40);
                    },
                    line=lines.next_line()=> {
                        let Some(line)=line.map_err(ApiError::internal)? else {break;};
                        let event:Value=serde_json::from_str(&line).map_err(|_|ApiError::bad("Invalid account bridge response"))?;
                        let is_view=event["type"]=="view";
                        let is_login=event["type"]=="login";
                        let reset_ack=if event["type"]=="reset_authorized" {Some(json!({"type":"reset_authorized_ack","idempotency_key":event["idempotency_key"]}).to_string())}else{None};
                        // Persist retry authorization before acknowledging it. The
                        // bridge cannot consume a credit until this durable write succeeds.
                        apply_event(state,&record,generation,event)?;
                        if let Some(ack)=reset_ack {
                            tokio::time::timeout(Duration::from_secs(2),input.write_all(format!("{ack}\n").as_bytes())).await.map_err(|_|ApiError::bad("Reset authorization acknowledgement timed out"))?.map_err(ApiError::internal)?;
                        }
                        received_view|=is_view;
                        if (is_view||is_login) && let Some(ready)=ready.take(){let _=ready.send(());}
                    }
                }
            }
            let status=tokio::time::timeout(Duration::from_secs(3),child.wait()).await.map_err(|_|ApiError::bad("Native account process did not exit"))?.map_err(ApiError::internal)?;
            if !received_view||!status.success(){return Err(ApiError::bad("Native account operation did not complete"));}
            Ok(())
        }.await;
        if io_result.is_err() { let _=tokio::time::timeout(Duration::from_millis(300),input.write_all(b"{\"type\":\"cancel\"}\n")).await; }
        close_group(&mut group, &mut child).await;
        protect_files(Path::new(&reference.config_dir))?;
        io_result
    }.await;
    if let Err(ref error) = result
        && let Ok(mut current) = view(state, id)
    {
        current.status = "error".into();
        current.login = None;
        current.error = Some(error.message.clone());
        current.checked_at = Some(chrono::Utc::now().to_rfc3339());
        let mut inner = state.accounts.inner.lock().expect("accounts lock");
        if inner.generations.get(&id) == Some(&generation) {
            inner.views.insert(id, current);
        }
    }
    if let Some(ready) = ready.take() {
        let _ = ready.send(());
    }
    result
}
fn apply_event(
    state: &AppState,
    record: &AccountRecord,
    generation: Uuid,
    event: Value,
) -> Result<()> {
    let mut current = view(state, record.id)?;
    match event["type"].as_str() {
        Some("login") => {
            let login: LoginView = serde_json::from_value(event["login"].clone())
                .map_err(|_| ApiError::bad("Invalid official login response"))?;
            let url = url::Url::parse(&login.url)
                .map_err(|_| ApiError::bad("Invalid official login URL"))?;
            let host = url.host_str().unwrap_or("");
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || !["openai.com", "chatgpt.com"]
                    .iter()
                    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
            {
                return Err(ApiError::bad(
                    "Login URL is not an official HTTPS authorization URL",
                ));
            }
            current.status = "login_pending".into();
            current.login = Some(login);
            current.error = None;
        }
        Some("view") => {
            let view: BridgeView = serde_json::from_value(event["view"].clone())
                .map_err(|_| ApiError::bad("Invalid official account state"))?;
            if !matches!(
                view.status.as_str(),
                "unknown" | "signed_out" | "signed_in" | "error"
            ) {
                return Err(ApiError::bad("Invalid account status"));
            }
            current.status = view.status;
            current.email = view.email;
            current.plan = view.plan;
            current.checked_at = view.checked_at;
            current.error = view.error;
            current.limits = view.limits;
            current.capabilities = view.capabilities;
            current.usage = view.usage;
            current.login = None;
        }
        Some("reset_authorized") => {
            let key = event["idempotency_key"]
                .as_str()
                .filter(|key| {
                    !key.is_empty() && key.len() <= 200 && !key.chars().any(char::is_control)
                })
                .ok_or_else(|| ApiError::bad("Invalid reset authorization"))?;
            if state
                .accounts
                .inner
                .lock()
                .expect("accounts lock")
                .generations
                .get(&record.id)
                != Some(&generation)
            {
                return Err(ApiError::conflict(
                    "Quota reset attempt is no longer active",
                ));
            }
            persist_reset_key(state, record.id, key)?;
        }
        Some("reset") => {
            let outcome = event["outcome"]
                .as_str()
                .filter(|outcome| {
                    matches!(
                        *outcome,
                        "reset" | "alreadyRedeemed" | "nothingToReset" | "noCredit"
                    )
                })
                .ok_or_else(|| ApiError::bad("Invalid official reset result"))?;
            current.reset_outcome = Some(outcome.into());
        }
        _ => return Err(ApiError::bad("Unsupported account bridge event")),
    }
    let mut inner = state.accounts.inner.lock().expect("accounts lock");
    if inner.generations.get(&record.id) == Some(&generation) {
        inner.views.insert(record.id, current);
    }
    Ok(())
}
