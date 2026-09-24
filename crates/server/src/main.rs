mod accounts;
#[cfg(test)]
mod api_tests;
mod canvas;
mod checkouts;
mod clients;
mod conversations;
mod daemon;
mod directories;
mod embedded;
mod environment;
mod file_search;
mod installation;
mod model_catalog;
mod native_config;
mod native_history;
mod providers;
mod resources;
mod security;
mod settings;
mod workspace_io;

use agentdock_domain::{
    EndpointProfile, InteractionMode, ProviderKind, Session, SessionId, SessionStatus, Workspace,
    WorkspaceId,
};
use agentdock_persistence::Store;
use agentdock_runtime::{RuntimeEvent, RuntimeManager, RuntimeSession};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
#[cfg(test)]
use tokio::net::TcpListener;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    runtime: RuntimeManager,
    state_dir: PathBuf,
    browse_roots: Vec<PathBuf>,
    workspace_roots: Vec<PathBuf>,
    native_sources: Vec<native_history::NativeSource>,
    native_bridge: PathBuf,
    chat_bridge: PathBuf,
    chats: conversations::ChatManager,
    accounts: accounts::AccountManager,
    security: security::Security,
    claude_manual_mode: bool,
    operations: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}
impl ApiError {
    fn bad(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
    fn missing(name: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("{name} not found"),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error,"operation failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Operation failed; see server logs".into(),
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error":self.message}))).into_response()
    }
}
impl From<workspace_io::IoError> for ApiError {
    fn from(error: workspace_io::IoError) -> Self {
        Self {
            status: StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message: error.message,
        }
    }
}
type Result<T> = std::result::Result<T, ApiError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWorkspace {
    name: String,
    root_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSession {
    interaction_mode: Option<InteractionMode>,
    #[serde(default)]
    environment: agentdock_domain::EnvironmentOverrides,
    provider: ProviderKind,
    title: String,
    endpoint_profile_id: Option<Uuid>,
    model: Option<String>,
    effort: Option<String>,
    /// Declared here or never. There is deliberately no way to mark an existing
    /// session temporary, only to promote a temporary one to permanent.
    #[serde(default)]
    ephemeral: bool,
}
#[derive(Deserialize, Default)]
struct SessionQuery {
    workspace_id: Option<WorkspaceId>,
}
#[derive(Deserialize, Default)]
struct PathQuery {
    path: Option<String>,
    #[serde(default)]
    staged: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProfile {
    #[serde(default)]
    environment: agentdock_domain::EnvironmentOverrides,
    name: String,
    provider: ProviderKind,
    endpoint_url: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    secret_ref: Option<String>,
    proxy_url: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    model_aliases: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportNativeProfile {
    source_id: String,
    name: Option<String>,
    #[serde(default)]
    confirmed_shared_config: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWrite {
    content: String,
    expected_version: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCreate {
    path: String,
}
/// Both ends, because a rename is a move: the tree offers it as renaming in
/// place, but the path is what is sent and a different folder is a valid one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRename {
    from: String,
    to: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitPaths {
    paths: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchInput {
    branch: String,
    #[serde(default)]
    create: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitInput {
    message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageInput {
    content: String,
}
#[derive(Deserialize, Default)]
struct PtyQuery {
    cols: Option<u16>,
    rows: Option<u16>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PtyCommand {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

/// Start the background gateway and report an address that actually answers.
///
/// A start that returned as soon as the process existed would print a URL that
/// is not listening yet, and would call a configuration error a success: the
/// detached process writes its failure to the log and exits, where nobody sees
/// it. So this waits for the health endpoint, and on timeout shows the log tail
/// that explains why rather than a URL that will not load.
async fn gateway_start(
    state_dir: &std::path::Path,
    address: SocketAddr,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if daemon::healthy(address).await {
        println!("AgentDock is already running at http://{address}/");
        return Ok(());
    }
    let pid = daemon::start(state_dir, address)?;
    if !daemon::wait_until_ready(address, Duration::from_secs(30)).await {
        let log = daemon::tail(state_dir, 20);
        let _ = daemon::stop(state_dir, Duration::from_secs(5));
        return Err(format!(
            "AgentDock did not become ready on {address}.\n\n{log}\n\nFull log: {}",
            daemon::log_file(state_dir).display()
        )
        .into());
    }
    println!("AgentDock {} (pid {pid})\n", env!("CARGO_PKG_VERSION"));
    print_access(state_dir, address);
    println!(
        "\n  logs    agentdock logs\n  stop    agentdock stop\n\nState: {}",
        state_dir.display()
    );
    Ok(())
}

/// Where to point a browser, and what it will ask for.
///
/// The token belongs here rather than only in the log: the gateway is detached,
/// so whatever it printed on its own stdout is somewhere nobody looks, and a URL
/// without the token it demands is only half an answer.
fn print_access(state_dir: &std::path::Path, address: SocketAddr) {
    for (label, url) in banner_urls(address) {
        println!("  {label}  {url}");
    }
    if !address.ip().is_loopback()
        && let Some(token) = security::stored_token(state_dir)
    {
        println!("  Token    {token}");
        // A deployment that has had its floor lowered says so every time it
        // starts. Weakening this is allowed; forgetting it is not.
        let minimum = security::minimum_token();
        if minimum < security::MINIMUM_TOKEN {
            println!("  Note     AGENTDOCK_TOKEN_MIN allows tokens of {minimum} characters here");
        }
    }
}

fn gateway_stop(
    state_dir: &std::path::Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    match daemon::stop(state_dir, Duration::from_secs(20))? {
        Some(pid) => println!("Stopped AgentDock (pid {pid})"),
        None => println!("AgentDock is not running"),
    }
    Ok(())
}

async fn gateway_status(
    state_dir: &std::path::Path,
    address: SocketAddr,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let pid = daemon::running(state_dir);
    let answering = daemon::healthy(address).await;
    match (pid, answering) {
        (Some(pid), true) => {
            println!("Running (pid {pid})\n");
            print_access(state_dir, address);
        }
        // A process that is up but silent is the shape a hung or still-starting
        // server takes, and saying "running" would hide it.
        (Some(pid), false) => println!("Process {pid} is up but {address} is not answering"),
        // Something else owns the port: reporting "not running" alone would be
        // true and useless when a start is about to fail on the bind.
        (None, true) => println!("Not started from this state directory, but {address} answers"),
        (None, false) => println!("Not running"),
    }
    Ok(())
}

/// The addresses worth printing for a given bind. A wildcard bind is reachable
/// at every interface, and the loopback name alone would not be the one someone
/// on the network needs.
fn banner_urls(address: SocketAddr) -> Vec<(&'static str, String)> {
    let mut urls = vec![("Local  ", format!("http://127.0.0.1:{}/", address.port()))];
    if address.ip().is_unspecified() {
        urls.push((
            "Network",
            format!("http://<this machine>:{}/", address.port()),
        ));
    } else if !address.ip().is_loopback() {
        urls[0] = ("Address", format!("http://{address}/"));
    }
    urls
}

/// Where the gateway listens unless told otherwise.
///
/// Deliberately not a common development port: 8787 collides with several
/// things a developer machine already runs, and a workspace that quietly failed
/// to bind is worse than one on a number nobody else claims.
const DEFAULT_ADDRESS: &str = "127.0.0.1:28789";

const HELP: &str = r#"AgentDock — a host workspace for Claude Code and Codex

  agentdock                 Start the gateway in the background
  agentdock --lan           Same, reachable from other machines on the network
  agentdock stop            Stop it
  agentdock restart         Stop it, then start it again
  agentdock status          Whether it is running, and where
  agentdock logs            What the background gateway has said
  agentdock serve           Run in the foreground instead (no background process)
  agentdock init            Create user state without starting anything

  --version                 Print the version
  --help                    This text

Settings live in <state>/config.toml, and an environment variable overrides the
file:

  lan = true              # reachable from other machines (same as --lan)
  port = 28789            # or addr = "192.168.0.9:28789"
  token = "..."           # AGENTDOCK_TOKEN
  token_min = 24          # shortest token this deployment accepts

Any other key names the AGENTDOCK_ variable it spells: shell, claude-bin,
instance-label, allowed-origins = ["http://..."], and so on.

Listens on 127.0.0.1:28789 otherwise. A binding that reaches other machines
needs an access token of at least 24 characters, which token_min can lower for
a network you trust; one is generated and kept in the state directory unless a
token is given, and `status` prints it again. State lives in ~/.agentdock;
AGENTDOCK_HOME or AGENTDOCK_STATE_DIR override it (that one cannot come from the
file, which lives inside it), and existing project-local .agentdock databases
are preserved."#;

/// Print failures as text and exit non-zero.
///
/// Returning the error from `main` formats it with `Debug`, which quotes the
/// whole message and prints its newlines as `\n` — so a startup failure that
/// carries a log tail arrived as one unreadable line. A supervisor also needs
/// the non-zero status to tell a failed start from a successful one.
fn main() {
    // The configuration file is read before the runtime is built, because
    // seeding the environment from it is only sound while this is the only
    // thread — and a runtime is what ends that.
    if let Err(error) = apply_settings() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    runtime.block_on(async {
        if let Err(error) = run().await {
            eprintln!("{error}");
            std::process::exit(1);
        }
    });
}

/// Let the file say what the environment has not.
///
/// A variable that is already set wins, because whoever set it did so later and
/// more deliberately than whoever wrote the file. A state directory that cannot
/// be resolved is not reported here: `run` says the same thing better, and
/// saying it twice from two places would be worse than once.
fn apply_settings() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let Ok(state_dir) = installation::state_directory() else {
        return Ok(());
    };
    let lan = env::args().any(|argument| argument == "--lan");
    for (key, value) in settings::pairs(&state_dir, lan)? {
        if env::var_os(&key).is_none() {
            // Sound here and nowhere later: no other thread exists yet.
            unsafe { env::set_var(&key, value) };
        }
    }
    Ok(())
}

async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Flags may precede or follow the command, so the command is the first
    // argument that is not one. `agentdock --lan` is a start with a flag, not a
    // request to run something called `--lan`.
    let action = env::args()
        .skip(1)
        .find(|argument| !argument.starts_with('-'))
        .unwrap_or_else(|| {
            if env::args().any(|argument| matches!(argument.as_str(), "--help" | "-h")) {
                "--help".into()
            } else if env::args().any(|argument| matches!(argument.as_str(), "--version" | "-V")) {
                "--version".into()
            } else {
                "start".into()
            }
        });
    if matches!(action.as_str(), "--help" | "-h" | "help") {
        println!("{HELP}");
        return Ok(());
    }
    if matches!(action.as_str(), "--version" | "-V" | "version") {
        println!("agentdock {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if !matches!(
        action.as_str(),
        "serve" | "init" | "start" | "stop" | "restart" | "status" | "logs"
    ) {
        return Err(format!("Unknown command {action:?}; use --help").into());
    }
    if action == "serve" {
        tracing_subscriber::fmt()
            .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| "agentdock=info".into()))
            .init();
    }
    let state_dir =
        providers::private_dir(&installation::state_directory()?).map_err(|e| e.message)?;
    let database = env::var("AGENTDOCK_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| state_dir.join("agentdock.db"));
    if action == "init" {
        let _database = installation::open_database(&state_dir, &database)?;
        println!(
            "Initialized AgentDock at {}\nDatabase: {}\nProjects and existing client credentials were not moved.",
            state_dir.display(),
            database.display()
        );
        return Ok(());
    }
    // `--lan` is the whole configuration for reaching this from another machine:
    // the bind, the token and the addresses to answer for all follow from it.
    let lan = env::args().any(|argument| argument == "--lan");
    let address: SocketAddr = match env::var("AGENTDOCK_ADDR") {
        Ok(value) => value.parse()?,
        Err(_) if lan => format!(
            "0.0.0.0:{}",
            DEFAULT_ADDRESS.rsplit(':').next().unwrap_or("")
        )
        .parse()?,
        Err(_) => DEFAULT_ADDRESS.parse()?,
    };
    match action.as_str() {
        "stop" => return gateway_stop(&state_dir),
        "status" => return gateway_status(&state_dir, address).await,
        "logs" => return Ok(daemon::print_log(&state_dir, 80)?),
        "start" | "restart" => {
            if action == "restart" {
                gateway_stop(&state_dir)?;
            }
            return gateway_start(&state_dir, address).await;
        }
        _ => {}
    }
    // Validate configuration and reserve both the port and database ownership
    // before any startup reconciliation can change existing session records.
    let token = security::resolve_token(&state_dir, !address.ip().is_loopback())?;
    let security = security::Security::new(address, token)?;
    let browse_roots = directories::roots()?;
    let workspace_roots = directories::workspace_roots(&browse_roots)?;
    let installation::PreparedServer {
        listener,
        database: locked_database,
    } = installation::prepare_server(address, &state_dir, &database).await?;
    let store = locked_database.store.clone();
    let claude_bin = clients::program(&state_dir, &agentdock_domain::ProviderKind::ClaudeCode);
    let help = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(claude_bin)
            .arg("--help")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let claude_manual_mode = matches!(help,Ok(Ok(ref output)) if String::from_utf8_lossy(&output.stdout).contains("\"manual\""));
    // Before anything resolves a bridge path: a single-file release carries the
    // Node modules inside it and has to place them first.
    installation::place_native_bridge(&state_dir)?;
    let state = AppState {
        store,
        runtime: RuntimeManager::new(),
        native_bridge: installation::native_bridge(&state_dir),
        chat_bridge: env::var_os("AGENTDOCK_CHAT_BRIDGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| installation::native_bridge(&state_dir).with_file_name("chat.mjs")),
        chats: conversations::ChatManager::default(),
        accounts: accounts::AccountManager::new(),
        state_dir,
        browse_roots,
        workspace_roots,
        native_sources: native_history::sources(),
        security,
        claude_manual_mode,
        operations: Arc::new(tokio::sync::Mutex::new(())),
    };
    let app = router(state.clone());
    // Reconcile only after startup has passed all configuration, binding and
    // ownership checks. `locked_database` stays alive through graceful shutdown.
    state.store.reconcile_after_restart()?;
    tracing::info!(%address,db=%database.display(),"AgentDock listening (trusted single-user host mode)");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            state.runtime.shutdown().await;
            state.chats.shutdown().await;
            state.accounts.shutdown().await;
        })
        .await?;
    Ok(())
}

fn router(state: AppState) -> Router {
    // A real build on disk wins, so a development tree or an installed layout
    // serves the files it just wrote. Otherwise the build that travels inside
    // this binary is served from memory. Existence decides it rather than the
    // override merely being set, so a path that points nowhere degrades to the
    // embedded copy instead of serving nothing at all.
    let web = installation::web_directory(&state.state_dir);
    let from_disk = web.join("index.html").exists();
    if !from_disk && !embedded::has_web() {
        tracing::warn!("no web assets on disk or in this binary; run the web build");
    }
    Router::new()
        .merge(accounts::routes())
        .merge(clients::routes())
        .merge(resources::routes())
        .merge(checkouts::routes())
        .merge(conversations::routes())
        .route("/api/health", get(health))
        .route("/api/auth", get(security::status).post(security::login))
        .route(
            "/api/canvas/layout",
            get(canvas::get_layout).put(canvas::save_layout),
        )
        .route("/api/host/directories", get(browse_directories))
        .route(
            "/api/host/native-configurations",
            get(native_configuration_sources),
        )
        .route("/api/native-history/sources", get(native_sources))
        .route(
            "/api/workspaces/{id}/native-history",
            get(list_native_history),
        )
        .route(
            "/api/workspaces/{id}/native-history/import",
            post(import_native_history),
        )
        .route(
            "/api/endpoint-profiles/discover-models",
            post(discover_models),
        )
        .route(
            "/api/endpoint-profiles/{id}/models",
            get(discover_profile_models),
        )
        .route(
            "/api/endpoint-profiles/import-native",
            post(import_native_profile),
        )
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route("/api/workspaces/{id}", get(get_workspace))
        .route("/api/workspaces/{id}/sessions", post(create_session))
        .route(
            "/api/workspaces/{id}/layout",
            get(get_layout).put(save_layout),
        )
        .route("/api/workspaces/{id}/files", get(list_files))
        .route("/api/workspaces/{id}/files/search", get(search_files))
        .route(
            "/api/workspaces/{id}/file",
            get(read_file).put(write_file).delete(delete_file),
        )
        .route("/api/workspaces/{id}/file/create", post(create_file))
        .route("/api/workspaces/{id}/file/rename", post(rename_file))
        .route(
            "/api/workspaces/{id}/attachments",
            // Raw bytes, so a phone upload costs no base64 inflation. The limit
            // is per-route: the shared 3 MiB JSON cap is for API payloads.
            post(upload_attachment).layer(DefaultBodyLimit::max(
                workspace_io::MAX_ATTACHMENT_BYTES as usize + 4096,
            )),
        )
        .route("/api/workspaces/{id}/asset", get(read_asset))
        .route("/api/workspaces/{id}/git/status", get(git_status))
        .route("/api/workspaces/{id}/git/diff", get(git_diff))
        .route("/api/workspaces/{id}/git/stage", post(git_stage))
        .route("/api/workspaces/{id}/git/unstage", post(git_unstage))
        .route("/api/workspaces/{id}/git/discard", post(git_discard))
        .route("/api/workspaces/{id}/git/branches", get(git_branches))
        .route("/api/workspaces/{id}/git/switch", post(git_switch))
        .route("/api/workspaces/{id}/git/commit", post(git_commit))
        .route(
            "/api/endpoint-profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/endpoint-profiles/{id}",
            get(get_profile)
                .patch(update_profile)
                .delete(delete_profile),
        )
        .route("/api/sessions", get(list_sessions))
        .route(
            "/api/sessions/{id}",
            get(get_session)
                .patch(update_session_title)
                .delete(discard_session),
        )
        .route("/api/sessions/{id}/keep", post(keep_session))
        .route(
            "/api/sessions/{id}/archive",
            axum::routing::patch(update_session_archive),
        )
        .route("/api/sessions/{id}/start", post(start_session))
        .route(
            "/api/sessions/{id}/terminal",
            post(open_session_in_terminal),
        )
        .route("/api/sessions/{id}/stop", post(stop_session))
        .route(
            "/api/sessions/{id}/environment",
            axum::routing::patch(update_session_environment),
        )
        .route("/api/sessions/{id}/message", post(send_message))
        .route("/api/sessions/{id}/pty/ws", get(session_socket))
        .route(
            "/api/{*path}",
            get(|| async { ApiError::missing("API route") }),
        )
        .fallback_service(if from_disk {
            axum::routing::any_service(
                ServeDir::new(&web).not_found_service(ServeFile::new(web.join("index.html"))),
            )
        } else {
            axum::routing::any(|uri: axum::http::Uri| async move { embedded::serve_web(&uri) })
        })
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::guard,
        ))
        .with_state(state)
}

async fn db<T: Send + 'static>(
    state: &AppState,
    operation: impl FnOnce(&Store) -> rusqlite_result::Result<T> + Send + 'static,
) -> Result<T> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || operation(&store))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}
// Keep rusqlite out of HTTP details while permitting typed persistence closures.
mod rusqlite_result {
    pub type Result<T> = std::result::Result<T, agentdock_persistence::DbError>;
}
async fn workspace(state: &AppState, id: WorkspaceId) -> Result<Workspace> {
    db(state, move |s| s.get_workspace(id))
        .await?
        .ok_or_else(|| ApiError::missing("Workspace"))
}
async fn session_record(state: &AppState, id: SessionId) -> Result<Session> {
    db(state, move |s| s.get_session(id))
        .await?
        .ok_or_else(|| ApiError::missing("Session"))
}
async fn root(state: &AppState, id: WorkspaceId) -> Result<PathBuf> {
    let w = workspace(state, id).await?;
    let path = tokio::fs::canonicalize(w.root_path)
        .await
        .map_err(|_| ApiError::conflict("Workspace directory is unavailable"))?;
    if !path.is_dir() {
        return Err(ApiError::conflict("Workspace root is not a directory"));
    }
    Ok(path)
}
fn bounded_name(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 200 {
        Err(ApiError::bad("Name must be 1–200 characters"))
    } else {
        Ok(s.into())
    }
}
fn optional(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}
async fn health() -> Json<Value> {
    Json(
        json!({"ok":true,"service":"agentdock-server","platform":env::consts::OS,"mode":"trusted-single-user","api_version":2,"capabilities":["shared_canvas","native_configurations","native_history","host_directories","endpoint_models","session_environment","structured_chat","official_accounts","session_configuration","session_archive","account_import_native","agent_clients","ephemeral_sessions","session_model","workspace_file_search","session_terminal_escape"],"instance_label":env::var("AGENTDOCK_INSTANCE_LABEL").ok(),"version":env!("CARGO_PKG_VERSION")}),
    )
}

#[derive(Deserialize, Default)]
struct DirectoryQuery {
    root: Option<String>,
    path: Option<String>,
}
async fn browse_directories(
    State(state): State<AppState>,
    Query(q): Query<DirectoryQuery>,
) -> Result<Json<directories::DirectoryListing>> {
    Ok(Json(
        directories::list(&state.browse_roots, q.root, q.path).await?,
    ))
}

async fn native_sources(State(state): State<AppState>) -> Json<Vec<native_history::SourceView>> {
    Json(native_history::source_views(&state.native_sources))
}
#[derive(Deserialize)]
struct HistoryQuery {
    source_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryImport {
    #[serde(default)]
    environment: Option<agentdock_domain::EnvironmentOverrides>,
    source_id: String,
    native_id: String,
    confirmed_original_config: bool,
}
async fn list_native_history(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<native_history::HistoryList>> {
    native_history::list(&state, id, &q.source_id)
        .await
        .map(Json)
}
async fn import_native_history(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<HistoryImport>,
) -> Result<Json<Session>> {
    if !input.confirmed_original_config {
        return Err(ApiError::bad(
            "Confirm using the original client configuration before loading history",
        ));
    }
    let session = match input.environment {
        Some(environment) => {
            native_history::import_with_environment(
                &state,
                id,
                input.source_id,
                input.native_id,
                Some(environment),
            )
            .await?
        }
        None => native_history::import(&state, id, input.source_id, input.native_id).await?,
    };
    Ok(Json(session))
}

async fn discover_models(
    State(state): State<AppState>,
    Json(input): Json<CreateProfile>,
) -> Result<Json<model_catalog::ModelCatalog>> {
    let p = EndpointProfile {
        id: Uuid::new_v4(),
        name: if input.name.trim().is_empty() {
            "Model discovery".into()
        } else {
            input.name
        },
        provider: input.provider,
        endpoint_url: optional(input.endpoint_url),
        model: optional(input.model),
        permission_mode: input.permission_mode.unwrap_or_else(|| "native".into()),
        secret_ref: optional(input.secret_ref),
        proxy_url: optional(input.proxy_url),
        effort: optional(input.effort),
        model_aliases: input.model_aliases,
        native_config: None,
        environment: input.environment,
        created_at: chrono::Utc::now(),
    };
    model_catalog::discover(&state, &p).await.map(Json)
}

/// Models a stored profile can run.
///
/// Official accounts are asked through the profile AgentDock already holds,
/// never through a native configuration supplied by the caller: the directory a
/// client is pointed at decides which credentials it reads.
async fn discover_profile_models(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<model_catalog::ModelCatalog>> {
    let profile = db(&state, move |s| s.get_endpoint_profile(id))
        .await?
        .ok_or_else(|| ApiError::missing("Profile"))?;
    model_catalog::discover(&state, &profile).await.map(Json)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentQuery {
    name: String,
}

/// Store an upload inside the workspace so a session can read it with its own
/// tools. This keeps attachments working for every client and file type, rather
/// than depending on one provider's inline-content protocol.
async fn upload_attachment(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(query): Query<AttachmentQuery>,
    body: axum::body::Bytes,
) -> Result<Json<workspace_io::Attachment>> {
    if query.name.trim().is_empty() || query.name.len() > 400 {
        return Err(ApiError::bad("Attachment needs a file name"));
    }
    let workspace = workspace(&state, id).await?;
    Ok(Json(
        workspace_io::write_attachment(
            std::path::Path::new(&workspace.root_path),
            &query.name,
            &body,
        )
        .await?,
    ))
}
async fn list_workspaces(State(state): State<AppState>) -> Result<Json<Vec<Workspace>>> {
    Ok(Json(db(&state, |s| s.list_workspaces()).await?))
}
async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<Json<Workspace>> {
    Ok(Json(workspace(&state, id).await?))
}
async fn create_workspace(
    State(state): State<AppState>,
    Json(input): Json<CreateWorkspace>,
) -> Result<(StatusCode, Json<Workspace>)> {
    let name = bounded_name(&input.name)?;
    let root_path = PathBuf::from(input.root_path);
    if !root_path.is_absolute() {
        return Err(ApiError::bad("Use an absolute host directory path"));
    }
    let canonical = tokio::fs::canonicalize(root_path)
        .await
        .map_err(|_| ApiError::bad("Host directory does not exist"))?;
    if !canonical.is_dir() {
        return Err(ApiError::bad("Root must be a directory"));
    }
    // Creating a workspace is what decides which part of the host this server
    // can read and write, so it is bounded like browsing is. Workspaces that
    // already exist are untouched: this is a rule about new ones.
    if !directories::within_roots(&state.workspace_roots, &canonical) {
        return Err(ApiError::bad(
            "That directory is outside the roots this server may use. Set AGENTDOCK_WORKSPACE_ROOTS to allow it.",
        ));
    }
    let path = canonical.to_string_lossy().into_owned();
    Ok((
        StatusCode::CREATED,
        Json(db(&state, move |s| s.create_workspace(&name, &path)).await?),
    ))
}
async fn list_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionQuery>,
) -> Result<Json<Vec<Session>>> {
    if let Some(id) = q.workspace_id {
        workspace(&state, id).await?;
    }
    Ok(Json(
        db(&state, move |s| s.list_sessions(q.workspace_id)).await?,
    ))
}
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Session>> {
    Ok(Json(session_record(&state, id).await?))
}
async fn create_session(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<CreateSession>,
) -> Result<(StatusCode, Json<Session>)> {
    environment::validate(&input.environment)?;
    workspace(&state, id).await?;
    let title = bounded_name(&input.title)?;
    let model = optional(input.model);
    let effort = optional(input.effort);
    providers::validate_effort(effort.as_deref())?;
    let interaction_mode = input.interaction_mode.unwrap_or_default();
    if input.provider == ProviderKind::Terminal && interaction_mode == InteractionMode::Structured {
        return Err(ApiError::bad(
            "Terminals do not support structured conversations",
        ));
    }
    if model
        .as_ref()
        .is_some_and(|m| m.len() > 200 || m.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Invalid model identifier"));
    }
    if input.provider == ProviderKind::Terminal && model.is_some() {
        return Err(ApiError::bad("Terminal sessions do not use a model"));
    }
    if let Some(pid) = input.endpoint_profile_id {
        let p = db(&state, move |s| s.get_endpoint_profile(pid))
            .await?
            .ok_or_else(|| ApiError::missing("Profile"))?;
        if p.provider != input.provider || input.provider == ProviderKind::Terminal {
            return Err(ApiError::bad("Profile and session providers must match"));
        }
        let mut effective = p.environment.clone();
        effective.extend(input.environment.clone());
        environment::validate(&effective)?;
        if let Some(reference) = p.native_config.as_ref() {
            // Model and reasoning depth are launch flags for this session only.
            // They select what the client runs and never rewrite the shared
            // native configuration, so they stay available here.
            let config_state = state.clone();
            let provider = p.provider.clone();
            let reference = reference.clone();
            tokio::task::spawn_blocking(move || {
                native_config::validate_reference(&config_state, &provider, &reference)
            })
            .await
            .map_err(ApiError::internal)??;
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(
            db(&state, move |s| {
                let session = s.create_session_with_configuration(
                    id,
                    input.provider,
                    &title,
                    input.endpoint_profile_id,
                    model,
                    effort,
                    input.environment,
                    input.ephemeral,
                )?;
                if interaction_mode == InteractionMode::Structured {
                    s.set_interaction_mode(session.id, interaction_mode)?;
                    s.get_session(session.id)?
                        .ok_or(agentdock_persistence::DbError::QueryReturnedNoRows)
                } else {
                    Ok(session)
                }
            })
            .await?,
        ),
    ))
}

/// Open the conversation a structured session owns in a real terminal client.
///
/// Structured mode drives the same client over a JSON pipe, which is precisely
/// what makes an interactive command such as `/config` unusable there. Rather
/// than approximating a TUI over that pipe, this creates a genuine terminal
/// session on the same native conversation: same client, same account, same
/// configuration home, same session id, but a real PTY the client can run any
/// interactive command in.
///
/// It is a separate session on purpose. The structured pane is left untouched,
/// so the escape hatch is an addition rather than a conversion, and closing the
/// terminal returns to structured mode with its history intact.
async fn open_session_in_terminal(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<(StatusCode, Json<Session>)> {
    let _guard = state.operations.lock().await;
    let source = session_record(&state, id).await?;
    if source.provider == ProviderKind::Terminal {
        return Err(ApiError::bad("This session is already a terminal"));
    }
    // An interactive reopen is only meaningful once the client has reported the
    // conversation it created; before that there is nothing to resume.
    let native_id = source
        .provider_session_id
        .as_deref()
        .filter(|value| native_history::valid_id(value))
        .ok_or_else(|| {
            ApiError::conflict("Send a message first; there is no conversation to reopen yet")
        })?
        .to_owned();
    let cwd = root(&state, source.workspace_id).await?;
    let title = terminal_title(&source.title);
    // The reopen shares the structured session's account and endpoint: it is the
    // same conversation, so it must reach the same place under the same terms.
    let (provider, profile_id, environment) = (
        source.provider.clone(),
        source.endpoint_profile_id,
        source.environment.clone(),
    );
    let record = db(&state, move |s| {
        // The reopen targets a conversation that already exists, so its native
        // id is known now -- unlike a fresh session, which learns it from the
        // client's first announcement.
        s.create_session_with_options(
            source.workspace_id,
            provider,
            &title,
            profile_id,
            None,
            None,
            environment,
            // Temporary: an escape hatch is opened to run one interactive
            // command and then closed. The conversation it carries is owned by
            // the structured session and survives there, so closing this window
            // should leave nothing behind. Keeping it is still one click away.
            true,
            Some(source.id),
            Some(native_id),
        )
    })
    .await?;
    // Creating a session does not launch anything here, as everywhere else: the
    // caller starts it through the ordinary route, which already knows how to
    // report a client that will not run. Resolving the launch now is only a
    // check that this session *can* be built, so an impossible reopen fails
    // before a record for it exists.
    let config_state = state.clone();
    let config_session = record.clone();
    tokio::task::spawn_blocking(move || providers::build(&config_state, &config_session, cwd))
        .await
        .map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, Json(record)))
}

/// A reopen is named after the conversation it reopens, so the sidebar shows it
/// beside the structured session rather than as an unrelated terminal.
fn terminal_title(title: &str) -> String {
    let base = format!("{} (terminal)", title.trim());
    base.chars().take(120).collect()
}

async fn start_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Session>> {
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    if session.interaction_mode == InteractionMode::Structured {
        conversations::start_locked(&state, &session).await?;
        return Ok(Json(session_record(&state, id).await?));
    }
    if state
        .runtime
        .get(&id.to_string())
        .is_some_and(|s| s.running())
    {
        return Ok(Json(session));
    }
    let cwd = checkouts::session_cwd(&state, &session).await?;
    let config_state = state.clone();
    let config_session = session.clone();
    let spec =
        tokio::task::spawn_blocking(move || providers::build(&config_state, &config_session, cwd))
            .await
            .map_err(ApiError::internal)??;
    db(&state, move |s| {
        s.set_session_status(id, SessionStatus::Starting)
    })
    .await?;
    let runtime = match state.runtime.start(id.to_string(), spec).await {
        Ok(runtime) => runtime,
        Err(_) => {
            db(&state, move |s| {
                s.set_session_result(
                    id,
                    SessionStatus::Failed,
                    Some("CLI could not start. Check executable path and native configuration."),
                )
            })
            .await?;
            return Err(ApiError::conflict(
                "CLI could not start. Check executable installation or AGENTDOCK_*_BIN.",
            ));
        }
    };
    db(&state, move |s| {
        s.set_session_status(id, SessionStatus::Running)
    })
    .await?;
    let watcher = state.clone();
    tokio::spawn(async move {
        let code = runtime.wait().await;
        let _lock = watcher.operations.lock().await;
        if watcher
            .runtime
            .get(&id.to_string())
            .is_some_and(|current| Arc::ptr_eq(&runtime, &current))
        {
            let _ = db(&watcher, move |s| {
                if s.get_session(id)?
                    .is_some_and(|record| matches!(record.status, SessionStatus::Stopped))
                {
                    return Ok(true); // Preserve an explicit stop's clean status.
                }
                if code == Some(0) {
                    s.set_session_result(id, SessionStatus::Stopped, None)
                } else {
                    s.set_session_result(
                        id,
                        SessionStatus::Stopped,
                        Some("Native process exited; restart explicitly or use native resume."),
                    )
                }
            })
            .await;
        }
    });
    Ok(Json(session_record(&state, id).await?))
}
async fn stop_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Session>> {
    let _guard = state.operations.lock().await;
    session_record(&state, id).await?;
    state.chats.stop(id).await?;
    if let Some(runtime) = state.runtime.get(&id.to_string()) {
        runtime
            .stop()
            .await
            .map_err(|_| ApiError::conflict("Could not stop process"))?;
    }
    db(&state, move |s| {
        s.set_session_status(id, SessionStatus::Stopped)
    })
    .await?;
    Ok(Json(session_record(&state, id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArchivePatch {
    archived: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionTitlePatch {
    title: String,
}

async fn update_session_title(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<SessionTitlePatch>,
) -> Result<Json<Session>> {
    session_record(&state, id).await?;
    let title = bounded_name(&input.title)?;
    Ok(Json(
        db(&state, move |store| store.update_session_title(id, &title))
            .await?
            .ok_or_else(|| ApiError::missing("Session"))?,
    ))
}

/// Promote a temporary session to a permanent one. Deliberately one-way: an
/// existing session can never be made destructible by closing its window.
async fn keep_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<Session>> {
    Ok(Json(
        db(&state, move |store| store.keep_session(id))
            .await?
            .ok_or_else(|| ApiError::missing("Session"))?,
    ))
}

/// Discard a temporary session: stop whatever it is running, then remove the
/// record. Refused for a permanent session, so the only way to reach this is to
/// have opted in when the session was created.
async fn discard_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> Result<Json<serde_json::Value>> {
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    if !session.ephemeral {
        return Err(ApiError::conflict(
            "Only a temporary session can be discarded. Archive a permanent session instead.",
        ));
    }
    state.chats.stop(id).await?;
    if let Some(runtime) = state.runtime.get(&id.to_string()) {
        runtime
            .stop()
            .await
            .map_err(|_| ApiError::conflict("Could not stop process"))?;
    }
    // Removing the record after the process is down keeps a discarded session
    // from leaving an orphaned client running with nothing bound to it.
    if !db(&state, move |store| store.delete_ephemeral_session(id)).await? {
        return Err(ApiError::missing("Session"));
    }
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_session_archive(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<SessionArchivePatch>,
) -> Result<Json<Session>> {
    let session = db(&state, move |store| {
        store.set_session_archived(id, input.archived)
    })
    .await?
    .ok_or_else(|| ApiError::missing("Session"))?;
    Ok(Json(session))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionEnvironmentPatch {
    environment: agentdock_domain::EnvironmentOverrides,
}

async fn update_session_environment(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<SessionEnvironmentPatch>,
) -> Result<Json<Session>> {
    let _guard = state.operations.lock().await;
    let session = session_record(&state, id).await?;
    if !matches!(
        session.status,
        SessionStatus::Stopped | SessionStatus::Failed
    ) || state.chats.get(id).is_some_and(|runtime| runtime.running())
        || state
            .runtime
            .get(&id.to_string())
            .is_some_and(|runtime| runtime.running())
    {
        return Err(ApiError::conflict(
            "Environment can only be edited while the session is stopped or failed",
        ));
    }
    environment::validate(&input.environment)?;
    if !db(&state, move |store| {
        store.update_session_environment(id, &input.environment)
    })
    .await?
    {
        return Err(ApiError::conflict(
            "Session state changed; environment was not updated",
        ));
    }
    Ok(Json(session_record(&state, id).await?))
}
async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(input): Json<MessageInput>,
) -> Result<StatusCode> {
    if session_record(&state, id).await?.interaction_mode == InteractionMode::Structured {
        return Err(ApiError::bad(
            "Use the structured conversation message endpoint with a request ID",
        ));
    }
    if input.content.is_empty() || input.content.len() > 32 * 1024 {
        return Err(ApiError::bad("Message must be 1–32768 bytes"));
    }
    let runtime = state
        .runtime
        .get(&id.to_string())
        .ok_or_else(|| ApiError::conflict("Start the session first"))?;
    runtime
        .input(format!("{}\r", input.content).into_bytes())
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
async fn session_socket(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Query(q): Query<PtyQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    session_record(&state, id).await?;
    if q.cols.is_some_and(|n| n == 0 || n > 1000) || q.rows.is_some_and(|n| n == 0 || n > 500) {
        return Err(ApiError::bad("Invalid terminal size"));
    }
    let runtime = state
        .runtime
        .get(&id.to_string())
        .ok_or_else(|| ApiError::conflict("No live runtime. Start the session explicitly."))?;
    Ok(upgrade
        .max_message_size(65536)
        .max_frame_size(65536)
        .on_upgrade(move |socket| stream_socket(socket, runtime, q)))
}

async fn stream_socket(socket: WebSocket, runtime: Arc<RuntimeSession>, q: PtyQuery) {
    let mut events = runtime.subscribe();
    let snapshot = runtime.snapshot();
    let (mut sender, mut receiver) = socket.split();
    let mut last = 0;
    let mut exited = false;
    for event in snapshot {
        let (seq, message, is_exit) = wire_event(event);
        if send_frame(&mut sender, message).await.is_err() {
            return;
        }
        last = seq;
        exited = is_exit;
    }
    if exited {
        let _ = sender.send(Message::Close(None)).await;
        return;
    }
    if let (Some(cols), Some(rows)) = (q.cols, q.rows) {
        let _ = runtime.resize(cols, rows).await;
    }
    loop {
        tokio::select! {
            event=events.recv()=>match event {
                Ok(event)=>{
                    let (seq,message,is_exit)=wire_event(event);
                    if seq<=last{continue;} last=seq;
                    if send_frame(&mut sender,message).await.is_err(){break;}
                    if is_exit{let _=sender.send(Message::Close(None)).await;break;}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{
                    let _=send_frame(&mut sender,Message::Text(json!({"type":"gap","message":"Output replay limit reached. Reconnect to the bounded history."}).to_string().into())).await;
                    let _=sender.send(Message::Close(None)).await;break;
                },
                Err(_)=>break,
            },
            message=receiver.next()=>match message {
                Some(Ok(Message::Binary(bytes)))=>{if let Err(error)=runtime.input(bytes.to_vec()).await{let _=send_frame(&mut sender,Message::Text(json!({"type":"error","message":error.to_string()}).to_string().into())).await;}},
                Some(Ok(Message::Text(text)))=>{
                    let result=match serde_json::from_str::<PtyCommand>(&text){
                        Ok(PtyCommand::Input{data})=>runtime.input(data.into_bytes()).await,
                        Ok(PtyCommand::Resize{cols,rows})=>runtime.resize(cols,rows).await,
                        Err(_)=>Err("Invalid terminal message".into()),
                    };
                    if let Err(error)=result {let _=send_frame(&mut sender,Message::Text(json!({"type":"error","message":error.to_string()}).to_string().into())).await;}
                },
                Some(Ok(Message::Ping(bytes)))=>{let _=sender.send(Message::Pong(bytes)).await;},
                Some(Ok(Message::Pong(_)))=>{},
                Some(Ok(Message::Close(_)))=>{let _=sender.send(Message::Close(None)).await;break;},
                None|Some(Err(_))=>break,
            }
        }
    }
    // A browser tab is a view, not the owner of the native process.
}
fn wire_event(event: RuntimeEvent) -> (u64, Message, bool) {
    match event {
        RuntimeEvent::Output { seq, data } => (seq, Message::Binary(data.into()), false),
        RuntimeEvent::Exited { seq, code } => (
            seq,
            Message::Text(json!({"type":"exit","code":code}).to_string().into()),
            true,
        ),
    }
}
async fn send_frame(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
) -> std::result::Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(5), sender.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn list_profiles(State(state): State<AppState>) -> Result<Json<Vec<EndpointProfile>>> {
    Ok(Json(db(&state, |s| s.list_endpoint_profiles()).await?))
}
async fn native_configuration_sources(
    State(state): State<AppState>,
) -> Result<Json<Vec<native_history::SourceView>>> {
    Ok(Json(
        tokio::task::spawn_blocking(move || native_config::source_views(&state))
            .await
            .map_err(ApiError::internal)?,
    ))
}
async fn import_native_profile(
    State(state): State<AppState>,
    Json(input): Json<ImportNativeProfile>,
) -> Result<Json<EndpointProfile>> {
    if !input.confirmed_shared_config {
        return Err(ApiError::bad(
            "Confirm sharing the original client configuration before importing it",
        ));
    }
    let config_state = state.clone();
    let (provider, reference) =
        tokio::task::spawn_blocking(move || native_config::pin(&config_state, &input.source_id))
            .await
            .map_err(ApiError::internal)??;
    let name = bounded_name(input.name.as_deref().unwrap_or(match provider {
        ProviderKind::Codex => "Host Codex configuration",
        ProviderKind::ClaudeCode => "Host Claude Code configuration",
        ProviderKind::Terminal => {
            return Err(ApiError::bad("Terminal has no native configuration"));
        }
    }))?;
    Ok(Json(
        db(&state, move |s| {
            s.import_native_profile(&name, provider, reference)
        })
        .await?,
    ))
}
async fn get_profile(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EndpointProfile>> {
    Ok(Json(
        db(&state, move |s| s.get_endpoint_profile(id))
            .await?
            .ok_or_else(|| ApiError::missing("Profile"))?,
    ))
}
async fn create_profile(
    State(state): State<AppState>,
    Json(input): Json<CreateProfile>,
) -> Result<(StatusCode, Json<EndpointProfile>)> {
    let p = EndpointProfile {
        id: Uuid::new_v4(),
        name: bounded_name(&input.name)?,
        provider: input.provider,
        endpoint_url: optional(input.endpoint_url),
        model: optional(input.model),
        permission_mode: input.permission_mode.unwrap_or_else(|| "native".into()),
        secret_ref: optional(input.secret_ref),
        proxy_url: optional(input.proxy_url),
        effort: optional(input.effort),
        model_aliases: input.model_aliases,
        native_config: None,
        environment: input.environment,
        created_at: chrono::Utc::now(),
    };
    providers::validate_profile(&p)?;
    let save = p.clone();
    db(&state, move |s| s.create_endpoint_profile(&save)).await?;
    Ok((StatusCode::CREATED, Json(p)))
}
async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<Value>,
) -> Result<Json<EndpointProfile>> {
    let mut p = db(&state, move |s| s.get_endpoint_profile(id))
        .await?
        .ok_or_else(|| ApiError::missing("Profile"))?;
    let map = input
        .as_object()
        .ok_or_else(|| ApiError::bad("Expected profile object"))?;
    if p.native_config.is_some() && map.keys().any(|key| key != "name" && key != "environment") {
        return Err(ApiError::bad(
            "An existing native configuration only allows name and session environment edits; original settings stay managed by the native client",
        ));
    }
    for (key, value) in map {
        if key == "environment" {
            p.environment = serde_json::from_value(value.clone())
                .map_err(|_| ApiError::bad("Invalid environment overrides"))?;
            continue;
        }
        if key == "model_aliases" {
            p.model_aliases = serde_json::from_value(value.clone())
                .map_err(|_| ApiError::bad("Model aliases must be a name-to-ID object"))?;
            continue;
        }
        let text = if value.is_null() {
            None
        } else {
            Some(
                value
                    .as_str()
                    .ok_or_else(|| ApiError::bad("Profile fields must be text or null"))?
                    .to_owned(),
            )
        };
        match key.as_str() {
            "name" => p.name = bounded_name(text.as_deref().unwrap_or(""))?,
            "endpoint_url" => p.endpoint_url = optional(text),
            "model" => p.model = optional(text),
            "secret_ref" => p.secret_ref = optional(text),
            "proxy_url" => p.proxy_url = optional(text),
            "effort" => p.effort = optional(text),
            "permission_mode" => {
                p.permission_mode = text.ok_or_else(|| ApiError::bad("Permission mode required"))?
            }
            "provider" => {
                if text.as_deref()
                    != Some(match p.provider {
                        ProviderKind::ClaudeCode => "claude_code",
                        ProviderKind::Codex => "codex",
                        ProviderKind::Terminal => "terminal",
                    })
                {
                    return Err(ApiError::bad("Create a new profile to change provider"));
                }
            }
            _ => return Err(ApiError::bad("Unknown profile field")),
        }
    }
    providers::validate_profile(&p)?;
    let save = p.clone();
    db(&state, move |s| s.update_endpoint_profile(&save)).await?;
    Ok(Json(p))
}
async fn delete_profile(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode> {
    let profile = db(&state, move |s| s.get_endpoint_profile(id))
        .await?
        .ok_or_else(|| ApiError::missing("Profile"))?;
    if profile
        .native_config
        .as_ref()
        .is_some_and(|reference| reference.source_id.starts_with("account:"))
        || accounts::owns_profile(&state, id)
    {
        return Err(ApiError::conflict(
            "Managed account profiles cannot be unlinked here. Maintain their sign-in from Official accounts; credential files are retained.",
        ));
    }
    if !db(&state, move |s| s.delete_endpoint_profile(id)).await? {
        return Err(ApiError::missing("Profile"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn search_files(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<file_search::SearchQuery>,
) -> Result<Json<file_search::SearchResults>> {
    Ok(Json(
        file_search::search(
            &state,
            id,
            q.q.as_deref().unwrap_or(""),
            q.limit.unwrap_or(20),
        )
        .await?,
    ))
}
async fn list_files(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Vec<workspace_io::FileEntry>>> {
    Ok(Json(
        workspace_io::list_files(&root(&state, id).await?, q.path.as_deref().unwrap_or("")).await?,
    ))
}
async fn read_file(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
) -> Result<Json<workspace_io::TextFile>> {
    Ok(Json(
        workspace_io::read_file(
            &root(&state, id).await?,
            q.path
                .as_deref()
                .ok_or_else(|| ApiError::bad("Path required"))?,
        )
        .await?,
    ))
}
async fn write_file(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
    Json(input): Json<FileWrite>,
) -> Result<Json<workspace_io::TextFile>> {
    let _guard = state.operations.lock().await;
    Ok(Json(
        workspace_io::write_file(
            &root(&state, id).await?,
            q.path
                .as_deref()
                .ok_or_else(|| ApiError::bad("Path required"))?,
            &input.content,
            input.expected_version.as_deref(),
        )
        .await?,
    ))
}
/// Add an empty file to a workspace. Refuses a name already taken rather than
/// writing over what holds it.
async fn create_file(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<FileCreate>,
) -> Result<(StatusCode, Json<workspace_io::FileEntry>)> {
    let _guard = state.operations.lock().await;
    let entry = workspace_io::create_file(&root(&state, id).await?, &input.path).await?;
    Ok((StatusCode::CREATED, Json(entry)))
}
/// Rename a file or directory within one workspace. Nothing is overwritten and
/// nothing leaves the workspace, so this is reversible by renaming back.
async fn rename_file(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<FileRename>,
) -> Result<Json<workspace_io::FileEntry>> {
    let _guard = state.operations.lock().await;
    Ok(Json(
        workspace_io::rename_entry(&root(&state, id).await?, &input.from, &input.to).await?,
    ))
}
/// Delete a file or directory a workspace owns. Destructive and not undoable,
/// so the caller states the path explicitly and the UI confirms it first.
async fn delete_file(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
) -> Result<StatusCode> {
    let _guard = state.operations.lock().await;
    workspace_io::delete_entry(
        &root(&state, id).await?,
        q.path
            .as_deref()
            .ok_or_else(|| ApiError::bad("Path required"))?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn read_asset(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
    request: Request,
) -> Result<Response> {
    let root = root(&state, id).await?;
    let target = workspace_io::asset_path(
        &root,
        q.path
            .as_deref()
            .ok_or_else(|| ApiError::bad("Path required"))?,
    )?;
    let mut response = ServeFile::new(target)
        .oneshot(request)
        .await
        .map_err(ApiError::internal)?
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        "sandbox; default-src 'none'; style-src 'unsafe-inline'"
            .parse()
            .unwrap(),
    );
    Ok(response)
}
async fn git_status(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<Json<workspace_io::GitStatus>> {
    Ok(Json(
        workspace_io::git_status(&root(&state, id).await?).await?,
    ))
}
async fn git_diff(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Query(q): Query<PathQuery>,
) -> Result<Json<workspace_io::GitDiff>> {
    Ok(Json(
        workspace_io::git_diff(&root(&state, id).await?, q.path.as_deref(), q.staged).await?,
    ))
}
async fn git_stage(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<GitPaths>,
) -> Result<StatusCode> {
    let _guard = state.operations.lock().await;
    workspace_io::git_stage(&root(&state, id).await?, &input.paths).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn git_unstage(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<GitPaths>,
) -> Result<StatusCode> {
    let _guard = state.operations.lock().await;
    workspace_io::git_unstage(&root(&state, id).await?, &input.paths).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn git_discard(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<GitPaths>,
) -> Result<StatusCode> {
    let _guard = state.operations.lock().await;
    workspace_io::git_discard(&root(&state, id).await?, &input.paths).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn git_branches(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<Json<workspace_io::GitBranches>> {
    let listed = workspace_io::git_branches(&root(&state, id).await?).await?;
    checkouts::refresh_labels(&state, id, &listed).await;
    Ok(Json(listed))
}
/// Switching branches rewrites the files under every session working in this
/// checkout, so it waits until none of them is running: an agent halfway
/// through an edit would otherwise find its files replaced underneath it.
async fn git_switch(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<BranchInput>,
) -> Result<StatusCode> {
    let _guard = state.operations.lock().await;
    let busy = db(&state, move |s| s.list_sessions(Some(id)))
        .await?
        .into_iter()
        .filter(|session| {
            matches!(
                session.status,
                SessionStatus::Running | SessionStatus::Starting | SessionStatus::Waiting
            )
        })
        .count();
    if busy > 0 {
        return Err(ApiError::conflict(format!(
            "{busy} session(s) are running in this workspace. End them, or open the branch in a new worktree instead."
        )));
    }
    workspace_io::git_switch(&root(&state, id).await?, input.branch.trim(), input.create).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn git_commit(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(input): Json<CommitInput>,
) -> Result<Json<Value>> {
    let _guard = state.operations.lock().await;
    Ok(Json(
        json!({"commit":workspace_io::git_commit(&root(&state,id).await?,&input.message).await?}),
    ))
}
async fn get_layout(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<Json<Value>> {
    workspace(&state, id).await?;
    let layout = db(&state, move |s| s.get_layout(id))
        .await?
        .map(|s| serde_json::from_str::<Value>(&s))
        .transpose()
        .map_err(ApiError::internal)?;
    // Legacy pre-ID M0 layouts cannot be rendered; return a valid default.
    Ok(Json(
        layout.filter(valid_layout).unwrap_or_else(default_layout),
    ))
}
async fn save_layout(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
    Json(layout): Json<Value>,
) -> Result<StatusCode> {
    workspace(&state, id).await?;
    if !valid_layout(&layout) || layout.to_string().len() > 256 * 1024 {
        return Err(ApiError::bad("Invalid or oversized layout"));
    }
    db(&state, move |s| s.save_layout(id, &layout.to_string())).await?;
    Ok(StatusCode::NO_CONTENT)
}
fn default_layout() -> Value {
    json!({"version":1,"root":{"type":"split","id":"root","direction":"horizontal","ratio":0.5,"first":{"type":"pane","id":"agent","kind":"agent_chat","title":"Agent"},"second":{"type":"pane","id":"changes","kind":"git_diff","title":"Changes"}}})
}
fn valid_layout(layout: &Value) -> bool {
    fn node(value: &Value, seen: &mut HashSet<String>, depth: usize, count: &mut usize) -> bool {
        *count += 1;
        if depth > 32 || *count > 512 {
            return false;
        }
        let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() < 200)
        else {
            return false;
        };
        if !seen.insert(id.to_owned()) {
            return false;
        }
        match value.get("type").and_then(Value::as_str) {
            Some("pane") => matches!(
                value.get("kind").and_then(Value::as_str),
                Some("agent_chat" | "terminal" | "editor" | "file_preview" | "git_diff")
            ),
            Some("split") => {
                value
                    .get("ratio")
                    .and_then(Value::as_f64)
                    .is_some_and(|n| (0.03..=0.97).contains(&n))
                    && matches!(
                        value.get("direction").and_then(Value::as_str),
                        Some("horizontal" | "vertical")
                    )
                    && value
                        .get("first")
                        .is_some_and(|n| node(n, seen, depth + 1, count))
                    && value
                        .get("second")
                        .is_some_and(|n| node(n, seen, depth + 1, count))
            }
            Some("stack") => value
                .get("panes")
                .and_then(Value::as_array)
                .is_some_and(|panes| {
                    panes.iter().all(|p| {
                        p.get("type") == Some(&json!("pane")) && node(p, seen, depth + 1, count)
                    })
                }),
            _ => false,
        }
    }
    layout.get("version") == Some(&json!(1))
        && layout
            .get("root")
            .is_some_and(|r| node(r, &mut HashSet::new(), 0, &mut 0))
}
