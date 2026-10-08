use super::*;
pub(super) use tower::ServiceExt;
#[path = "archive_api_tests.rs"]
mod archive_tests;
#[path = "conversation_api_tests.rs"]
mod conversation_tests;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use std::{collections::BTreeMap, fs, process::Command};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as WsMessage, client::IntoClientRequest},
};

/// A native process that stays alive until it is stopped: `/bin/sh` with
/// `unix_args`, or on Windows an interactive `cmd.exe` waiting for input.
fn waiting_process(unix_args: &[&str]) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd.exe".into(), vec!["/Q".into()])
    } else {
        (
            "/bin/sh".into(),
            unix_args.iter().map(|arg| (*arg).to_owned()).collect(),
        )
    }
}

/// Terminal input that makes a [`waiting_process`] print `{first}_{second}_READY`.
/// The typed text never contains that sentinel, so the terminal's echo of the
/// input cannot satisfy a test. On Windows it first answers the cursor query
/// ConPTY opens with, as xterm.js does in the app.
fn announce(first: &str, second: &str) -> String {
    if cfg!(windows) {
        format!("\x1b[1;1Recho {first}_^{second}_READY\r")
    } else {
        format!("printf '{first}_%s_READY\\n' '{second}'\r")
    }
}

struct Fixture {
    state: AppState,
    path: PathBuf,
}
impl Fixture {
    fn new(address: SocketAddr, token: Option<&str>) -> Self {
        let path = std::env::temp_dir().join(format!("agentdock-api-{}", Uuid::new_v4()));
        fs::create_dir_all(path.join("repo")).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "AgentDock test"],
            vec!["config", "user.email", "test@agentdock.invalid"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path.join("repo"))
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let state = AppState {
            store: Arc::new(Store::open(":memory:").unwrap()),
            runtime: RuntimeManager::new(),
            state_dir: path.join("state"),
            browse_roots: vec![dunce::canonicalize(path.join("repo")).unwrap()],
            host_grants: crate::host_grants::HostGrants::default(),
            workspace_roots: vec![dunce::canonicalize(&path).unwrap()],
            native_sources: Vec::new(),
            native_bridge: PathBuf::from("packages/native-bridge/history.mjs"),
            chat_bridge: PathBuf::from("packages/native-bridge/chat.mjs"),
            chats: conversations::ChatManager::default(),
            watches: file_watch::FileWatches::default(),
            accounts: accounts::AccountManager::default(),
            security: security::Security::for_test(address, token),
            claude_manual_mode: true,
            operations: Arc::new(tokio::sync::Mutex::new(())),
            agents: agent::AgentRegistry::default(),
            activity: activity::Activity::default(),
            metrics: None,
        };
        Self { state, path }
    }
    fn app(&self) -> Router {
        router(self.state.clone())
    }

    /// The test bridge reads only this fixture's static JSON. It never imports
    /// a provider SDK, launches a native client, or inspects the user's history.
    fn native_source(&mut self, provider: ProviderKind, items: Vec<Value>) -> (String, PathBuf) {
        let source_id = format!("fixture-native-{}", self.state.native_sources.len());
        let config = self.path.join(&source_id);
        fs::create_dir(&config).unwrap();
        let config = dunce::canonicalize(config).unwrap();
        fs::write(
            config.join("history-fixture.json"),
            serde_json::to_vec(&json!({"items":items,"truncated":false})).unwrap(),
        )
        .unwrap();
        let bridge = self.path.join("fixture-native-history.mjs");
        fs::write(
            &bridge,
            r#"import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const job = JSON.parse(input);
writeFileSync(join(job.config_dir, 'bridge-used'), 'fixture only');
const items = readFileSync(join(job.config_dir, 'history-fixture.json'), 'utf8');
process.stdout.write(items);
"#,
        )
        .unwrap();
        self.state.native_bridge = bridge;
        self.state
            .native_sources
            .push(native_history::NativeSource {
                id: source_id.clone(),
                label: "Test fixture only".into(),
                provider,
                config_dir: config.clone(),
                config_env: Some(config.as_os_str().to_owned()),
            });
        (source_id, config)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
async fn call(app: Router, method: &str, path: &str, value: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8787")
        .header("x-agentdock-client", "web")
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn register(f: &Fixture) -> String {
    let (status, w) = call(
        f.app(),
        "POST",
        "/api/workspaces",
        json!({"name":"fixture","root_path":f.path.join("repo")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    w["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn workspace_file_git_layout_workflow() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let base = format!("/api/workspaces/{id}");
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/workspaces",
            json!({"name":"bad","root_path":"/not-an-agentdock-directory"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, file) = call(
        f.app(),
        "PUT",
        &format!("{base}/file?path=hello.txt"),
        json!({"content":"one\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(
            f.app(),
            "PUT",
            &format!("{base}/file?path=hello.txt"),
            json!({"content":"two\n","expected_version":"stale"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            f.app(),
            "PUT",
            &format!("{base}/file?path=hello.txt"),
            json!({"content":"two\n","expected_version":file["version"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, diff) = call(
        f.app(),
        "GET",
        &format!("{base}/git/diff?path=hello.txt"),
        Value::Null,
    )
    .await;
    assert!(diff["diff"].as_str().unwrap().contains("+two"));
    assert_eq!(
        call(
            f.app(),
            "POST",
            &format!("{base}/git/stage"),
            json!({"paths":["hello.txt"]})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            &format!("{base}/git/unstage"),
            json!({"paths":["hello.txt"]})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(f.path.join("repo/hello.txt").exists());
    call(
        f.app(),
        "POST",
        &format!("{base}/git/stage"),
        json!({"paths":["hello.txt"]}),
    )
    .await;
    let (status, result) = call(
        f.app(),
        "POST",
        &format!("{base}/git/commit"),
        json!({"message":"fixture commit"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["commit"].as_str().unwrap().len(), 40);
    let (_, status) = call(f.app(), "GET", &format!("{base}/git/status"), Value::Null).await;
    assert!(status["files"].as_array().unwrap().is_empty());
    // A worktree is part of its workspace: made beside the repository, never
    // registered as a workspace of its own.
    let (status, tree) = call(
        f.app(),
        "POST",
        &format!("{base}/git/worktrees"),
        json!({"branch":"feature/tree","create":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tree}");
    let tree_path = tree["path"].as_str().unwrap().to_owned();
    assert!(std::path::Path::new(&tree_path).ends_with("repo.worktrees/feature-tree"));
    assert_eq!(tree["branch"], "feature/tree");
    let (_, listed) = call(f.app(), "GET", "/api/workspaces", Value::Null).await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "no second workspace");
    // Asking again finds the same worktree rather than making another.
    let (_, again) = call(
        f.app(),
        "POST",
        &format!("{base}/git/worktrees"),
        json!({"branch":"feature/tree"}),
    )
    .await;
    assert_eq!(again["path"], tree["path"]);
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/worktrees"),
        json!({"path":"/tmp"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A session moves to that checkout on its own; the workspace is untouched.
    let (_, session) = call(
        f.app(),
        "POST",
        &format!("{base}/sessions"),
        json!({"provider":"claude_code","title":"On a branch"}),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_owned();
    let (status, moved) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{sid}/checkout"),
        json!({"branch":"feature/tree"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["checkout_path"].as_str(), Some(tree_path.as_str()));
    assert_eq!(moved["checkout_branch"], "feature/tree");
    assert_eq!(moved["configuration_revision"], 1, "a new native context");
    // A branch renamed inside its worktree is followed: the session keeps its
    // directory and shows the new name.
    assert!(
        std::process::Command::new("git")
            .args(["branch", "-m", "feature/tree", "feature/renamed"])
            .current_dir(f.path.join("repo"))
            .status()
            .unwrap()
            .success()
    );
    call(f.app(), "GET", &format!("{base}/git/branches"), Value::Null).await;
    let (_, renamed) = call(f.app(), "GET", &format!("/api/sessions/{sid}"), Value::Null).await;
    assert_eq!(renamed["checkout_branch"], "feature/renamed");
    assert_eq!(renamed["checkout_path"].as_str(), Some(tree_path.as_str()));
    assert_eq!(
        renamed["configuration_revision"], 1,
        "a label, not a new context"
    );
    // Renaming through the API is followed the same way.
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/branches/rename"),
        json!({"from":"feature/renamed","to":"feature/final"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, followed) = call(f.app(), "GET", &format!("/api/sessions/{sid}"), Value::Null).await;
    assert_eq!(followed["checkout_branch"], "feature/final");
    // Choosing the workspace's own branch brings it back to the workspace.
    let (_, home) = call(f.app(), "GET", &format!("{base}/git/branches"), Value::Null).await;
    let current = home["current"].as_str().unwrap().to_owned();
    let (_, back) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{sid}/checkout"),
        json!({"branch":current}),
    )
    .await;
    assert!(back["checkout_path"].is_null());
    // Removing a worktree moves the stopped sessions bound to it home first.
    call(
        f.app(),
        "POST",
        &format!("/api/sessions/{sid}/checkout"),
        json!({"branch":"feature/final"}),
    )
    .await;
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/worktrees/remove"),
        json!({"path":tree_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, homed) = call(f.app(), "GET", &format!("/api/sessions/{sid}"), Value::Null).await;
    assert!(homed["checkout_path"].is_null());
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/branches/delete"),
        json!({"branch":"feature/final","force":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, conversation) = call(
        f.app(),
        "GET",
        &format!("/api/sessions/{sid}/conversation"),
        Value::Null,
    )
    .await;
    let boundaries = conversation["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "configuration")
        .count();
    assert_eq!(boundaries, 4, "each move is marked in the conversation");
    // A session running in the workspace directory holds the switch until
    // the caller asks for it to be ended.
    f.state
        .store
        .set_session_status(sid.parse().unwrap(), SessionStatus::Running)
        .unwrap();
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/switch"),
        json!({"branch":"side","create":true}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("{base}/git/switch"),
        json!({"branch":"side","create":true,"stop_sessions":true}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, ended) = call(f.app(), "GET", &format!("/api/sessions/{sid}"), Value::Null).await;
    assert_eq!(ended["status"], "stopped");
    let layout = default_layout();
    assert_eq!(
        call(f.app(), "PUT", &format!("{base}/layout"), layout.clone())
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(f.app(), "GET", &format!("{base}/layout"), Value::Null)
            .await
            .1,
        layout
    );
    assert_eq!(
        call(
            f.app(),
            "PUT",
            &format!("{base}/layout"),
            json!({"version":1,"root":{"id":"a","type":"split"}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f.app(),
            "GET",
            &format!("{base}/file?path=../state"),
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

/// A launch's arguments without the ones that give it AgentDock's own tools
/// (agent::equip), for tests about everything else a launch carries.
fn without_agentdock(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg.starts_with("--mcp-config=") || arg == "--allowedTools=mcp__agentdock" {
            index += 1;
        } else if arg == "-c"
            && args
                .get(index + 1)
                .is_some_and(|value| value.starts_with("mcp_servers.agentdock."))
        {
            index += 2;
        } else {
            kept.push(arg.clone());
            index += 1;
        }
    }
    kept
}

#[tokio::test]
async fn every_client_launch_carries_agentdock_tools_unless_turned_off() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = f
        .state
        .store
        .create_workspace("equip", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let cwd = f.path.join("repo");
    let claude = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::ClaudeCode, "claude")
        .unwrap();
    let spec = providers::build(&f.state, &claude, cwd.clone()).unwrap();
    let token = spec.env["AGENTDOCK_AGENT_TOKEN"].clone();
    let config = spec
        .args
        .iter()
        .find_map(|arg| arg.strip_prefix("--mcp-config="))
        .expect("mcp config flag");
    assert!(
        spec.args
            .contains(&"--allowedTools=mcp__agentdock".to_owned())
    );
    let written: Value = serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    let server = &written["mcpServers"]["agentdock"];
    assert_eq!(server["args"], json!(["mcp"]));
    assert_eq!(server["env"]["AGENTDOCK_AGENT_TOKEN"], token);
    assert_eq!(server["env"]["AGENTDOCK_SESSION_ID"], claude.id.to_string());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // A relaunch replaces the token; the old one names no one.
    let again = providers::build(&f.state, &claude, cwd.clone()).unwrap();
    assert_ne!(again.env["AGENTDOCK_AGENT_TOKEN"], token);

    let codex = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Codex, "codex")
        .unwrap();
    let spec = providers::build(&f.state, &codex, cwd.clone()).unwrap();
    assert_eq!(spec.args[0], "-c");
    assert_eq!(
        spec.args[1],
        "mcp_servers.agentdock.command=".to_owned()
            + &serde_json::to_string(&std::env::current_exe().unwrap().to_string_lossy()).unwrap()
    );
    let env = spec
        .args
        .iter()
        .find(|arg| arg.starts_with("mcp_servers.agentdock.env="))
        .unwrap();
    assert!(env.contains(&format!(
        "AGENTDOCK_AGENT_TOKEN={}",
        serde_json::to_string(&spec.env["AGENTDOCK_AGENT_TOKEN"]).unwrap()
    )));
    assert!(
        spec.args
            .contains(&"mcp_servers.agentdock.tool_timeout_sec=600".to_owned())
    );

    let terminal = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Terminal, "shell")
        .unwrap();
    let spec = providers::build(&f.state, &terminal, cwd.clone()).unwrap();
    assert!(spec.env.contains_key("AGENTDOCK_AGENT_TOKEN"));
    assert_eq!(without_agentdock(&spec.args), spec.args);

    let (status, _) = call(
        f.app(),
        "PUT",
        "/api/preferences",
        json!({"agent_tools": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, preferences) = call(f.app(), "GET", "/api/preferences", Value::Null).await;
    assert_eq!(preferences["agent_tools"], false);
    let spec = providers::build(&f.state, &claude, cwd).unwrap();
    assert!(!spec.env.keys().any(|key| key.starts_with("AGENTDOCK_")));
    assert_eq!(without_agentdock(&spec.args), spec.args);
}

/// A request to the agent tools the way `agentdock mcp` makes one.
async fn agent_request(
    app: Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    client: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8787")
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(client) = client {
        request = request.header("x-agentdock-client", client);
    }
    let response = app
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn tool_text(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null)
}

#[tokio::test]
async fn agent_tools_know_their_caller_and_a_session_token_opens_nothing_else() {
    const DEPLOYMENT: &str = "deployment-token-for-agent-tests-0123456789";
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), Some(DEPLOYMENT));
    let workspace = f
        .state
        .store
        .create_workspace("agents", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let session = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Terminal, "worker")
        .unwrap();
    let spec = providers::build(&f.state, &session, f.path.join("repo")).unwrap();
    let token = spec.env["AGENTDOCK_AGENT_TOKEN"].clone();
    assert_eq!(spec.env["AGENTDOCK_SESSION_ID"], session.id.to_string());
    assert_eq!(spec.env["AGENTDOCK_URL"], "http://127.0.0.1:8787");
    let status = json!({"name":"agentdock_status"});

    // Issued but not running: the token identifies no one yet.
    let (code, _) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        Some("agent"),
        status.clone(),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);

    f.state
        .runtime
        .start(
            session.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: f.path.join("repo"),
                env: BTreeMap::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .unwrap();

    let (code, tools) = agent_request(
        f.app(),
        "GET",
        "/api/agent/tools",
        Some(&token),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(
        tools["instructions"]
            .as_str()
            .unwrap()
            .contains("agentdock_status")
    );
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"agentdock_status") && names.contains(&"agentdock_list"));

    let (code, result) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        Some("agent"),
        status.clone(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(result["isError"], false);
    let you = &tool_text(&result)["you"];
    assert_eq!(you["kind"], "session");
    assert_eq!(you["session"]["id"], session.id.to_string());
    assert_eq!(you["workspace"]["name"], "agents");

    // The session's token reaches the agent tools and nothing else.
    for (method, path) in [
        ("GET", "/api/sessions"),
        ("GET", "/api/secrets"),
        ("GET", "/api/endpoint-profiles"),
    ] {
        let (code, _) = agent_request(
            f.app(),
            method,
            path,
            Some(&token),
            Some("web"),
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "{path}");
    }
    // A mutation still has to say it comes from an agent (or the web app).
    let (code, _) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        None,
        status.clone(),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    // `agent` is no password for the rest of the API.
    let (code, _) = agent_request(
        f.app(),
        "POST",
        "/api/workspaces",
        Some(DEPLOYMENT),
        Some("agent"),
        json!({"name":"x","root_path":"/"}),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    // The deployment's own token is an external caller.
    let (_, result) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(DEPLOYMENT),
        Some("agent"),
        status.clone(),
    )
    .await;
    assert_eq!(tool_text(&result)["you"]["kind"], "external");
    let (_, unknown) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(DEPLOYMENT),
        Some("agent"),
        json!({"name":"agentdock_nothing"}),
    )
    .await;
    assert_eq!(unknown["isError"], true);

    // Listing endpoints says how a key is set, never what it is.
    crate::secrets::store(
        &f.state.state_dir,
        "AGENTDOCK_SECRET_AGENT_LIST",
        "sk-never-listed",
    )
    .unwrap();
    let (_, profile) = agent_request(f.app(), "POST", "/api/endpoint-profiles", Some(DEPLOYMENT), Some("web"), json!({"name":"listed","provider":"codex","endpoint_url":"https://example.test/v1","secret_ref":"env:AGENTDOCK_SECRET_AGENT_LIST"})).await;
    assert_eq!(profile.get("error"), None, "{profile}");
    let (_, listed) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        Some("agent"),
        json!({"name":"agentdock_list","arguments":{"what":"endpoints"}}),
    )
    .await;
    assert!(!listed.to_string().contains("sk-never-listed"));
    assert_eq!(tool_text(&listed)[0]["key"], "saved");
    let (_, sessions) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        Some("agent"),
        json!({"name":"agentdock_list","arguments":{"what":"sessions"}}),
    )
    .await;
    assert_eq!(tool_text(&sessions)[0]["title"], "worker");

    // Once the process ends, so does the token.
    f.state.runtime.stop(&session.id.to_string()).await.unwrap();
    for _ in 0..50 {
        if !f
            .state
            .runtime
            .get(&session.id.to_string())
            .is_some_and(|r| r.running())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (code, _) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        Some(&token),
        Some("agent"),
        status,
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

/// The next event of one type from the activity stream, as a browser gets it.
async fn next_event(events: &mut tokio::sync::broadcast::Receiver<Value>, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == kind {
                return event;
            }
        }
    })
    .await
    .expect("activity event")
}

#[tokio::test]
async fn an_agent_endpoint_waits_for_the_person_and_its_key_never_passes_through_the_agent() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let create = json!({"name":"agentdock_endpoint","arguments":{"action":"create","name":"Team relay","provider":"codex","endpoint_url":"https://relay.example.test/v1","model":"gpt-test"}});

    // No window open: nobody can answer, so the agent is told at once.
    let (_, result) = agent_request(
        f.app(),
        "POST",
        "/api/agent/call",
        None,
        Some("agent"),
        create.clone(),
    )
    .await;
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No AgentDock window")
    );

    let mut events = f.state.activity.subscribe();
    let app = f.app();
    let pending = tokio::spawn({
        let create = create.clone();
        async move { agent_request(app, "POST", "/api/agent/call", None, Some("agent"), create).await }
    });
    let request = next_event(&mut events, "request").await;
    assert_eq!(request["title"], "Create endpoint profile “{name}”");
    assert_eq!(request["values"]["name"], "Team relay");
    assert_eq!(request["key"]["optional"], false);
    assert!(
        request["rows"].as_array().unwrap().iter().any(
            |row| row["label"] == "Endpoint" && row["value"] == "https://relay.example.test/v1"
        )
    );
    let id = request["id"].as_str().unwrap().to_owned();
    // A key it asked for is required to approve.
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("/api/agent-activity/requests/{id}"),
        json!({"approve":true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("/api/agent-activity/requests/{id}"),
        json!({"approve":true,"secret":"sk-typed-in-agentdock"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, result) = pending.await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    assert!(!result.to_string().contains("sk-typed-in-agentdock"));
    let created = &tool_text(&result)["created"];
    assert_eq!(created["key"], "saved");
    assert_eq!(created["model"], "gpt-test");
    assert_eq!(
        crate::secrets::resolve(&f.state.state_dir, "AGENTDOCK_SECRET_TEAM_RELAY").as_deref(),
        Some("sk-typed-in-agentdock")
    );
    let notice = next_event(&mut events, "notice").await;
    assert_eq!(notice["message"], "Created endpoint profile “{name}”");
    assert_eq!(notice["undoable"], true);
    assert_eq!(notice["who"]["kind"], "external");

    // Undo takes the profile and its key away again.
    let (status, _) = call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/notices/{}/undo",
            notice["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, profiles) = call(f.app(), "GET", "/api/endpoint-profiles", Value::Null).await;
    assert_eq!(profiles, json!([]));
    assert!(crate::secrets::resolve(&f.state.state_dir, "AGENTDOCK_SECRET_TEAM_RELAY").is_none());

    // Declining changes nothing, and says so.
    let app = f.app();
    let call_again = tokio::spawn({
        let create = create.clone();
        async move { agent_request(app, "POST", "/api/agent/call", None, Some("agent"), create).await }
    });
    let request = next_event(&mut events, "request").await;
    call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/requests/{}",
            request["id"].as_str().unwrap()
        ),
        json!({"approve":false}),
    )
    .await;
    let (_, result) = call_again.await.unwrap();
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("declined")
    );
    let (_, profiles) = call(f.app(), "GET", "/api/endpoint-profiles", Value::Null).await;
    assert_eq!(profiles, json!([]));
}

#[tokio::test]
async fn defaults_change_at_once_with_an_undo_and_only_running_unasked_is_confirmed() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let mut events = f.state.activity.subscribe();
    let (_, result) = agent_request(f.app(), "POST", "/api/agent/call", None, Some("agent"), json!({"name":"agentdock_preferences","arguments":{"action":"set","provider":"claude_code","effort":"high"}})).await;
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(tool_text(&result)["saved"]["claude_code"]["effort"], "high");
    let notice = next_event(&mut events, "notice").await;
    call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/notices/{}/undo",
            notice["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    let (_, preferences) = call(f.app(), "GET", "/api/preferences", Value::Null).await;
    assert_eq!(preferences["claude_code"].get("effort"), None);

    let app = f.app();
    let danger = tokio::spawn(async move {
        agent_request(app, "POST", "/api/agent/call", None, Some("agent"), json!({"name":"agentdock_preferences","arguments":{"action":"set","provider":"codex","permission":"danger"}})).await
    });
    let request = next_event(&mut events, "request").await;
    assert_eq!(request["values"]["client"], "Codex");
    call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/requests/{}",
            request["id"].as_str().unwrap()
        ),
        json!({"approve":true}),
    )
    .await;
    assert_eq!(danger.await.unwrap().1["isError"], false);

    let (_, added) = agent_request(f.app(), "POST", "/api/agent/call", None, Some("agent"), json!({"name":"agentdock_workspace","arguments":{"action":"add","path":f.path.join("repo")}})).await;
    assert_eq!(tool_text(&added)["added"]["name"], "repo");
}

#[tokio::test]
async fn a_session_token_can_neither_see_nor_answer_the_persons_questions() {
    const DEPLOYMENT: &str = "deployment-token-for-activity-tests-012345";
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), Some(DEPLOYMENT));
    let workspace = f
        .state
        .store
        .create_workspace("q", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let session = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Terminal, "asker")
        .unwrap();
    let token = providers::build(&f.state, &session, f.path.join("repo"))
        .unwrap()
        .env["AGENTDOCK_AGENT_TOKEN"]
        .clone();
    f.state
        .runtime
        .start(
            session.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: f.path.join("repo"),
                env: BTreeMap::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .unwrap();
    let mut events = f.state.activity.subscribe();
    let app = f.app();
    let asking = tokio::spawn({
        let token = token.clone();
        async move {
            agent_request(app, "POST", "/api/agent/call", Some(&token), Some("agent"), json!({"name":"agentdock_endpoint","arguments":{"action":"create","name":"x","provider":"codex","key":"none"}})).await
        }
    });
    let request = next_event(&mut events, "request").await;
    assert_eq!(request["who"]["kind"], "session");
    assert_eq!(request["who"]["title"], "asker");
    let id = request["id"].as_str().unwrap();
    for client in ["agent", "web"] {
        let (status, _) = agent_request(
            f.app(),
            "POST",
            &format!("/api/agent-activity/requests/{id}"),
            Some(&token),
            Some(client),
            json!({"approve":true}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{client}");
    }
    let (status, _) = agent_request(
        f.app(),
        "GET",
        "/api/agent-activity/ws",
        Some(&token),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The person can.
    let (status, _) = agent_request(
        f.app(),
        "POST",
        &format!("/api/agent-activity/requests/{id}"),
        Some(DEPLOYMENT),
        Some("web"),
        json!({"approve":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(asking.await.unwrap().1["isError"], false);
    f.state.runtime.stop(&session.id.to_string()).await.unwrap();
}

#[tokio::test]
async fn a_key_saved_in_agentdock_reaches_the_client_and_never_comes_back() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let name = "AGENTDOCK_SECRET_API_TEST_STORED";
    let (status, saved) = call(
        f.app(),
        "PUT",
        &format!("/api/secrets/{name}"),
        json!({"value":"sk-stored-value"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, json!({"name":name,"source":"agentdock"}));
    let (status, listed) = call(f.app(), "GET", "/api/secrets", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == name)
    );
    assert!(!listed.to_string().contains("sk-stored-value"));
    let (status, _) = call(f.app(), "PUT", "/api/secrets/HOME", json!({"value":"x"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, profile) = call(f.app(), "POST", "/api/endpoint-profiles", json!({"name":"stored","provider":"codex","endpoint_url":"https://example.test/v1","secret_ref":format!("env:{name}")})).await;
    let (_, created) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{id}/sessions"),
        json!({"title":"uses stored key","provider":"codex","endpoint_profile_id":profile["id"]}),
    )
    .await;
    let session: Session = serde_json::from_value(created).unwrap();
    let spec = providers::build(&f.state, &session, f.path.join("repo")).unwrap();
    assert_eq!(
        spec.env.get("OPENAI_API_KEY").map(String::as_str),
        Some("sk-stored-value")
    );

    let (status, removed) = call(
        f.app(),
        "DELETE",
        &format!("/api/secrets/{name}"),
        Value::Null,
    )
    .await;
    assert_eq!((status, removed), (StatusCode::OK, json!({"removed":true})));
    let error = providers::build(&f.state, &session, f.path.join("repo")).unwrap_err();
    assert!(error.message.contains(name), "{}", error.message);
}

#[tokio::test]
async fn profiles_validate_snapshot_and_session_states() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let (_,p)=call(f.app(),"POST","/api/endpoint-profiles",json!({"name":"work","provider":"codex","endpoint_url":"https://example.test/v1","permission_mode":"interactive"})).await;
    let pid = p["id"].as_str().unwrap();
    let (status, s) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{id}/sessions"),
        json!({"title":"one","provider":"codex","endpoint_profile_id":pid}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(s["status"], "stopped");
    assert_eq!(
        call(
            f.app(),
            "POST",
            &format!("/api/workspaces/{id}/sessions"),
            json!({"title":"wrong","provider":"claude_code","endpoint_profile_id":pid})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &format!("/api/endpoint-profiles/{pid}"),
            json!({"endpoint_url":null})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, unchanged) = call(
        f.app(),
        "GET",
        &format!("/api/sessions/{}", s["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(
        unchanged["endpoint_snapshot"]["endpoint_url"],
        "https://example.test/v1"
    );
    let record = f
        .state
        .store
        .get_session(s["id"].as_str().unwrap().parse().unwrap())
        .unwrap()
        .unwrap();
    let spec = providers::build(&f.state, &record, f.path.join("repo")).unwrap();
    let config =
        fs::read_to_string(PathBuf::from(&spec.env["CODEX_HOME"]).join("config.toml")).unwrap();
    let parsed: toml::Value = toml::from_str(&config).unwrap();
    assert_eq!(
        parsed["model_providers"]["agentdock"]["base_url"].as_str(),
        Some("https://example.test/v1")
    );
    assert!(
        spec.args
            .contains(&"approval_policy=\"on-request\"".to_owned())
    );
    assert!(!config.contains("never"));
    let sid = s["id"].as_str().unwrap();
    assert_eq!(
        call(
            f.app(),
            "POST",
            &format!("/api/sessions/{sid}/stop"),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &format!("/api/sessions/{sid}"),
            json!({"status":"running"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn claude_plan_mode_is_native_and_codex_does_not_fake_it() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (status, profile) = call(
        f.app(),
        "POST",
        "/api/endpoint-profiles",
        json!({"name":"Claude plan","provider":"claude_code","permission_mode":"plan"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, session) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{workspace}/sessions"),
        json!({"title":"plan session","provider":"claude_code","endpoint_profile_id":profile["id"]}),
    ).await;
    let record = f
        .state
        .store
        .get_session(session["id"].as_str().unwrap().parse().unwrap())
        .unwrap()
        .unwrap();
    let spec = providers::build(&f.state, &record, f.path.join("repo")).unwrap();
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "plan"])
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/endpoint-profiles",
            json!({"name":"Codex plan","provider":"codex","permission_mode":"plan"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_temporary_session_is_declared_at_creation_and_only_then_can_be_discarded() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let sessions_path = format!("/api/workspaces/{workspace}/sessions");
    let create = |body: Value| call(f.app(), "POST", &sessions_path, body);

    let (_, temporary) =
        create(json!({"title":"Scratch","provider":"claude_code","ephemeral":true})).await;
    let (_, permanent) = create(json!({"title":"Keep me","provider":"claude_code"})).await;
    let temporary_id = temporary["id"].as_str().unwrap().to_owned();
    let permanent_id = permanent["id"].as_str().unwrap().to_owned();
    assert_eq!(temporary["ephemeral"], true);
    // Omitting the field is the ordinary, permanent case.
    assert_eq!(permanent["ephemeral"], false);

    // An unarchived session cannot be deleted directly; a permanent one must
    // be archived first.
    assert_eq!(
        call(
            f.app(),
            "DELETE",
            &format!("/api/sessions/{permanent_id}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        f.state
            .store
            .get_session(permanent_id.parse().unwrap())
            .unwrap()
            .is_some()
    );

    // Promotion is one-way: once kept, the destructive path closes behind it.
    let (status, kept) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{temporary_id}/keep"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kept["ephemeral"], false);
    assert_eq!(kept["id"], temporary_id);
    assert_eq!(
        call(
            f.app(),
            "DELETE",
            &format!("/api/sessions/{temporary_id}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // There is deliberately no request shape that marks an existing session
    // temporary again.
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &format!("/api/sessions/{temporary_id}"),
            json!({"ephemeral":true})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let (_, throwaway) =
        create(json!({"title":"Second scratch","provider":"terminal","ephemeral":true})).await;
    let throwaway_id = throwaway["id"].as_str().unwrap().to_owned();
    let (status, discarded) = call(
        f.app(),
        "DELETE",
        &format!("/api/sessions/{throwaway_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(discarded["id"], throwaway_id);
    assert!(
        f.state
            .store
            .get_session(throwaway_id.parse().unwrap())
            .unwrap()
            .is_none()
    );
    // A second discard reports the record is gone rather than succeeding twice.
    assert_eq!(
        call(
            f.app(),
            "DELETE",
            &format!("/api/sessions/{throwaway_id}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_permanent_session_can_be_deleted_only_once_archived() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (_, permanent) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{workspace}/sessions"),
        json!({"title":"Old work","provider":"terminal"}),
    )
    .await;
    let id = permanent["id"].as_str().unwrap().to_owned();
    let path = format!("/api/sessions/{id}");
    assert_eq!(
        call(f.app(), "DELETE", &path, Value::Null).await.0,
        StatusCode::CONFLICT
    );
    let (status, _) = call(
        f.app(),
        "PATCH",
        &format!("/api/sessions/{id}/archive"),
        json!({"archived":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, deleted) = call(f.app(), "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["id"], id);
    assert!(
        f.state
            .store
            .get_session(id.parse().unwrap())
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_restart_clears_temporary_sessions_and_keeps_every_permanent_one() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let sessions_path = format!("/api/workspaces/{workspace}/sessions");
    let create = |body: Value| call(f.app(), "POST", &sessions_path, body);
    let (_, temporary) =
        create(json!({"title":"Scratch","provider":"claude_code","ephemeral":true})).await;
    let (_, permanent) = create(json!({"title":"Keep me","provider":"claude_code"})).await;

    f.state.store.reconcile_after_restart().unwrap();

    // A temporary session cannot outlive its process, so it goes rather than
    // becoming another stopped row in the list.
    assert!(
        f.state
            .store
            .get_session(temporary["id"].as_str().unwrap().parse().unwrap())
            .unwrap()
            .is_none()
    );
    let survivor = f
        .state
        .store
        .get_session(permanent["id"].as_str().unwrap().parse().unwrap())
        .unwrap()
        .unwrap();
    assert!(matches!(survivor.status, SessionStatus::Stopped));
    assert!(!survivor.ephemeral);
}

#[tokio::test]
async fn session_title_can_be_renamed_without_changing_identity_or_process_state() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (_, created) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{workspace}/sessions"),
        json!({"title":"Initial Claude task","provider":"claude_code"}),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    let (status, renamed) = call(
        f.app(),
        "PATCH",
        &format!("/api/sessions/{id}"),
        json!({"title":"Review billing"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["id"], id);
    assert_eq!(renamed["title"], "Review billing");
    assert_eq!(renamed["status"], "stopped");
    let record = f
        .state
        .store
        .get_session(id.parse().unwrap())
        .unwrap()
        .unwrap();
    let spec = providers::build(&f.state, &record, f.path.join("repo")).unwrap();
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--name", "Review billing"])
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &format!("/api/sessions/{id}"),
            json!({"title":"  "})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            "/api/sessions/not-a-session",
            json!({"title":"Nope"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn proxy_alias_and_session_model_override_are_snapshotted() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let (status,p)=call(f.app(),"POST","/api/endpoint-profiles",json!({"name":"mapped","provider":"codex","proxy_url":"http://127.0.0.1:7890","model":"fast","effort":"high","model_aliases":{"fast":"actual-model-id"}})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_,s)=call(f.app(),"POST",&format!("/api/workspaces/{id}/sessions"),json!({"title":"mapped session","provider":"codex","endpoint_profile_id":p["id"],"model":"fast","effort":"low"})).await;
    let record = f
        .state
        .store
        .get_session(s["id"].as_str().unwrap().parse().unwrap())
        .unwrap()
        .unwrap();
    let spec = providers::build(&f.state, &record, f.path.join("repo")).unwrap();
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--model", "actual-model-id"])
    );
    assert!(
        spec.args
            .iter()
            .any(|arg| arg == "model_reasoning_effort=\"low\"")
    );
    assert_eq!(spec.env["HTTPS_PROXY"], "http://127.0.0.1:7890");
    assert_eq!(spec.env["https_proxy"], "http://127.0.0.1:7890");
    let pid = p["id"].as_str().unwrap();
    call(
        f.app(),
        "PATCH",
        &format!("/api/endpoint-profiles/{pid}"),
        json!({"proxy_url":null,"model_aliases":{},"effort":"max"}),
    )
    .await;
    let snapshot = record.endpoint_snapshot.as_ref().unwrap();
    assert_eq!(snapshot.model_aliases["fast"], "actual-model-id");
    assert_eq!(snapshot.effort.as_deref(), Some("low"));
    let (_, dirs) = call(f.app(), "GET", "/api/host/directories", Value::Null).await;
    assert_eq!(dirs["root_id"], "0");
    assert_eq!(
        call(
            f.app(),
            "GET",
            "/api/host/directories?root=0&path=../",
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn origin_auth_and_asset_range() {
    let f = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("fixture-token-long-enough"),
    );
    let app = f.app();
    assert_eq!(
        call(app.clone(), "GET", "/api/workspaces", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let attack = Request::builder()
        .method("POST")
        .uri("/api/workspaces")
        .header("host", "127.0.0.1:8787")
        .header("origin", "https://attacker.invalid")
        .header("x-agentdock-client", "web")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(attack).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let login = Request::builder()
        .method("POST")
        .uri("/api/auth")
        .header("host", "127.0.0.1:8787")
        .header("x-agentdock-client", "web")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"token":"fixture-token-long-enough"}).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(login).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("HttpOnly")
    );
    let w = f
        .state
        .store
        .create_workspace("range", f.path.join("repo").to_str().unwrap())
        .unwrap();
    fs::write(f.path.join("repo/image.png"), b"0123456789").unwrap();
    let req = Request::builder()
        .uri(format!("/api/workspaces/{}/asset?path=image.png", w.id))
        .header("host", "127.0.0.1:8787")
        .header("authorization", "Bearer fixture-token-long-enough")
        .header("range", "bytes=2-5")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        &to_bytes(response.into_body(), 100).await.unwrap()[..],
        b"2345"
    );
}

#[tokio::test]
async fn file_socket_reports_changes_on_disk() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let f = Fixture::new(addr, None);
    let repo = f.path.join("repo");
    fs::create_dir(repo.join("src")).unwrap();
    let w = f
        .state
        .store
        .create_workspace("watch", repo.to_str().unwrap())
        .unwrap();
    let app = f.app();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut req = format!("ws://{addr}/api/workspaces/{}/files/ws", w.id)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("origin", format!("http://{addr}").parse().unwrap());
    let (mut ws, _) = connect_async(req).await.unwrap();
    async fn message(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("file socket message")
            {
                Some(Ok(WsMessage::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(_)) => continue,
                other => panic!("file socket ended: {other:?}"),
            }
        }
    }
    assert_eq!(message(&mut ws).await["type"], "ready");
    fs::write(repo.join("src/new.ts"), "export {};\n").unwrap();
    fs::create_dir(repo.join("node_modules")).unwrap();
    let mut seen = Vec::new();
    while !seen.iter().any(|path| path == "src/new.ts") {
        let change = message(&mut ws).await;
        assert_eq!(change["type"], "changed");
        seen.extend(
            change["paths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|path| path.as_str().unwrap().to_owned()),
        );
    }
    assert!(!seen.iter().any(|path| path.starts_with("node_modules")));
    server.abort();
}

#[tokio::test]
async fn websocket_detach_reconnect_retains_native_process() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let f = Fixture::new(addr, None);
    let w = f
        .state
        .store
        .create_workspace("pty", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let s = f
        .state
        .store
        .create_session(w.id, ProviderKind::Terminal, "shell")
        .unwrap();
    let native = f
        .state
        .runtime
        .start(
            s.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: f.path.join("repo"),
                env: BTreeMap::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .unwrap();
    let app = f.app();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("ws://{addr}/api/sessions/{}/pty/ws", s.id);
    let mut req = url.clone().into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", format!("http://{addr}").parse().unwrap());
    let (mut ws, _) = connect_async(req).await.unwrap();
    ws.send(WsMessage::Text(
        json!({"type":"input","data":if cfg!(windows) { announce("AD", "SOCKET") } else { format!("stty -echo; {}", announce("AD", "SOCKET")) }})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok(event)) = ws.next().await {
            if let WsMessage::Binary(data) = event {
                bytes.extend_from_slice(&data);
                if String::from_utf8_lossy(&bytes).contains("AD_SOCKET_READY") {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("AD_SOCKET_READY"));
    ws.close(None).await.unwrap();
    drop(ws);
    assert!(native.running());
    let (mut ws, _) = connect_async(url.clone()).await.unwrap();
    let mut replay = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok(event)) = ws.next().await {
            if let WsMessage::Binary(data) = event {
                replay.extend_from_slice(&data);
                if String::from_utf8_lossy(&replay).contains("AD_SOCKET_READY") {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&replay).contains("AD_SOCKET_READY"));
    assert!(Arc::ptr_eq(
        &native,
        &f.state.runtime.get(&s.id.to_string()).unwrap()
    ));
    ws.close(None).await.ok();

    // A framed view numbers what it receives and can resume after it.
    let control = |message: &WsMessage| match message {
        WsMessage::Text(text) => serde_json::from_str::<Value>(text).ok(),
        _ => None,
    };
    let (mut ws, _) = connect_async(format!("{url}?framed=true")).await.unwrap();
    let hello = control(&ws.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello["type"], "stream");
    assert_eq!(hello["resumed"], false);
    let instance = hello["runtime"].as_str().unwrap().to_owned();
    let mut seen = 0u64;
    let mut framed = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok(WsMessage::Binary(data))) = ws.next().await {
            let seq = u64::from_be_bytes(data[..8].try_into().unwrap());
            assert!(seq > seen, "frames arrive in sequence order");
            seen = seq;
            framed.extend_from_slice(&data[8..]);
            if String::from_utf8_lossy(&framed).contains("AD_SOCKET_READY") {
                break;
            }
        }
    })
    .await
    .unwrap();
    ws.close(None).await.ok();
    drop(ws);
    let resume = format!("{url}?framed=true&runtime={instance}&after={seen}");
    let (mut ws, _) = connect_async(resume).await.unwrap();
    let hello = control(&ws.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello["resumed"], true);
    ws.send(WsMessage::Text(json!({"type":"ping"}).to_string().into()))
        .await
        .unwrap();
    ws.send(WsMessage::Text(
        json!({"type":"input","data":announce("AD", "RESUME")})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let mut ponged = false;
    let mut after = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok(event)) = ws.next().await {
            match event {
                WsMessage::Binary(data) => {
                    let seq = u64::from_be_bytes(data[..8].try_into().unwrap());
                    assert!(seq > seen, "a resumed view is not sent what it already has");
                    after.extend_from_slice(&data[8..]);
                    if String::from_utf8_lossy(&after).contains("AD_RESUME_READY") {
                        break;
                    }
                }
                other => ponged |= control(&other).is_some_and(|v| v["type"] == "pong"),
            }
        }
    })
    .await
    .unwrap();
    assert!(ponged);
    // The sequence numbers above are the guarantee. ConPTY repaints the whole
    // screen on Windows, so earlier text can reappear there as new output.
    if !cfg!(windows) {
        assert!(!String::from_utf8_lossy(&after).contains("AD_SOCKET_READY"));
    }
    ws.close(None).await.ok();
    drop(ws);
    // Another process's numbering is not this one's: the view starts over.
    let (mut ws, _) = connect_async(format!("{url}?framed=true&runtime=other&after={seen}"))
        .await
        .unwrap();
    let hello = control(&ws.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello["resumed"], false);
    native.stop().await.unwrap();
    ws.close(None).await.ok();
    server.abort();
}

#[tokio::test]
async fn native_history_requires_confirmation_and_filters_cross_workspace_imports() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    fs::create_dir(f.path.join("other-repo")).unwrap();
    let other_cwd = dunce::canonicalize(f.path.join("other-repo")).unwrap();
    let other = f
        .state
        .store
        .create_workspace("other fixture", other_cwd.to_str().unwrap())
        .unwrap();
    let (source_id, config) = f.native_source(ProviderKind::Codex, vec![
        json!({"id":"native-allowed","provider":"codex","title":"Fixture thread","cwd":cwd,"updated_at":"2026-09-09T00:00:00Z"}),
    ]);
    let import_path = format!("/api/workspaces/{workspace}/native-history/import");
    let (status, rejection) = call(
        f.app(),
        "POST",
        &import_path,
        json!({"source_id":source_id,"native_id":"native-allowed","confirmed_original_config":false}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rejection["error"].as_str().unwrap().contains("Confirm"));
    assert!(
        !config.join("bridge-used").exists(),
        "No bridge should run before consent"
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            &import_path,
            json!({"source_id":source_id,"native_id":"native-allowed"}),
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(!config.join("bridge-used").exists());
    assert_eq!(call(
        f.app(), "POST", &import_path,
        json!({"source_id":source_id,"native_id":"../native-allowed","confirmed_original_config":true}),
    ).await.0, StatusCode::BAD_REQUEST);
    assert!(!config.join("bridge-used").exists());

    let (status, rejection) = call(
        f.app(), "POST", &format!("/api/workspaces/{}/native-history/import", other.id),
        json!({"source_id":source_id,"native_id":"native-allowed","confirmed_original_config":true}),
    ).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{rejection}");
    assert!(config.join("bridge-used").exists());
    assert!(f.state.store.list_sessions(None).unwrap().is_empty());

    let (status, imported) = call(
        f.app(), "POST", &import_path,
        json!({"source_id":source_id,"native_id":"native-allowed","confirmed_original_config":true}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["workspace_id"], workspace);
    assert_eq!(imported["provider_session_id"], "native-allowed");
    assert_eq!(imported["status"], "stopped");
    assert!(imported.get("native_config_dir").is_none());
    let (_, duplicate) = call(
        f.app(), "POST", &import_path,
        json!({"source_id":source_id,"native_id":"native-allowed","confirmed_original_config":true}),
    ).await;
    assert_eq!(duplicate["id"], imported["id"]);
    assert_eq!(f.state.store.list_sessions(None).unwrap().len(), 1);
    assert!(
        f.state
            .runtime
            .get(imported["id"].as_str().unwrap())
            .is_none(),
        "Import must not launch or attach any process"
    );
}

#[tokio::test]
async fn native_history_list_is_metadata_only_and_registry_identity_is_authoritative() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    fs::create_dir(f.path.join("unrelated-repo")).unwrap();
    let unrelated = dunce::canonicalize(f.path.join("unrelated-repo")).unwrap();
    let (source_id, config) = f.native_source(ProviderKind::ClaudeCode, vec![
        json!({"id":"claude-thread","provider":"claude_code","title":"长".repeat(300),"cwd":cwd,"updated_at":"2026-09-09T00:00:00Z","imported_session_id":"not-authoritative","api_key":"DO_NOT_EXPOSE_FAKE_KEY","messages":[{"content":"DO_NOT_EXPOSE_FAKE_CONTENT"}],"config_dir":"DO_NOT_EXPOSE_FAKE_CONFIG"}),
        json!({"id":"unrelated-thread","provider":"claude_code","title":"other","cwd":unrelated,"updated_at":"2026-09-09T00:00:00Z"}),
        json!({"id":"wrong-provider","provider":"codex","title":"wrong","cwd":cwd,"updated_at":"2026-09-09T00:00:00Z"}),
        json!({"id":"../path-id","provider":"claude_code","title":"invalid","cwd":cwd,"updated_at":"2026-09-09T00:00:00Z"}),
    ]);
    let list_path = format!("/api/workspaces/{workspace}/native-history?source_id={source_id}");
    let (status, listing) = call(f.app(), "GET", &list_path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    let item = &listing["items"][0];
    assert_eq!(item["id"], "claude-thread");
    assert_eq!(item["title"].as_str().unwrap().chars().count(), 240);
    assert!(item["imported_session_id"].is_null());
    for field in ["api_key", "messages", "config_dir"] {
        assert!(
            item.get(field).is_none(),
            "Unexpected private field: {field}"
        );
    }
    assert!(!listing.to_string().contains("DO_NOT_EXPOSE"));
    let managed = f
        .state
        .store
        .import_native_session(
            workspace.parse().unwrap(),
            ProviderKind::ClaudeCode,
            "Managed",
            &source_id,
            "claude-thread",
            config.to_str().unwrap(),
        )
        .unwrap();
    let (_, listing) = call(f.app(), "GET", &list_path, Value::Null).await;
    assert_eq!(
        listing["items"][0]["imported_session_id"],
        managed.id.to_string()
    );
    let (_, public_session) = call(
        f.app(),
        "GET",
        &format!("/api/sessions/{}", managed.id),
        Value::Null,
    )
    .await;
    assert!(public_session.get("native_config_dir").is_none());
    assert!(
        !public_session
            .to_string()
            .contains(config.to_str().unwrap())
    );

    let missing = format!("/api/workspaces/{workspace}/native-history?source_id=not-registered");
    assert_eq!(
        call(f.app(), "GET", &missing, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn native_history_resume_preserves_original_configuration_and_rejects_source_changes() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("resume fixture", cwd.to_str().unwrap())
        .unwrap();
    for (provider, expected_args, config_key) in [
        (
            ProviderKind::Codex,
            vec!["resume", "fixture-id"],
            "CODEX_HOME",
        ),
        (
            ProviderKind::ClaudeCode,
            vec!["--resume", "fixture-id"],
            "CLAUDE_CONFIG_DIR",
        ),
    ] {
        let (source_id, config) = f.native_source(provider.clone(), Vec::new());
        let session = f
            .state
            .store
            .import_native_session(
                workspace.id,
                provider.clone(),
                "Resume fixture",
                &source_id,
                "fixture-id",
                config.to_str().unwrap(),
            )
            .unwrap();
        let spec = native_history::resume_spec(&f.state, &session, cwd.clone()).unwrap();
        assert_eq!(spec.args, expected_args);
        assert_eq!(spec.cwd, cwd);
        assert_eq!(spec.env[config_key], config.to_string_lossy());
        assert_eq!(
            spec.env.len(),
            1,
            "Do not inject endpoint, account or permission overrides when resuming original history"
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("bypass")
            || arg.contains("dangerously")
            || arg.contains("--last")));
        assert!(f.state.runtime.get(&session.id.to_string()).is_none());
        assert!(
            !config.join("bridge-used").exists(),
            "Constructing resume arguments must not execute a client"
        );

        let mut invalid_id = session.clone();
        invalid_id.provider_session_id = Some("--all".into());
        assert_eq!(
            native_history::resume_spec(&f.state, &invalid_id, cwd.clone())
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );
        let mut wrong_provider = session.clone();
        wrong_provider.provider = ProviderKind::Terminal;
        assert_eq!(
            native_history::resume_spec(&f.state, &wrong_provider, cwd.clone())
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );

        let replacement = f.path.join(format!("replacement-{source_id}"));
        fs::create_dir(&replacement).unwrap();
        f.state
            .native_sources
            .iter_mut()
            .find(|source| source.id == source_id)
            .unwrap()
            .config_dir = replacement;
        let rejected = native_history::resume_spec(&f.state, &session, cwd.clone()).unwrap_err();
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        assert!(rejected.message.contains("source changed"));
    }
}

#[tokio::test]
async fn opening_a_structured_session_in_a_terminal_resumes_its_own_configuration_home() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("fixture", cwd.to_str().unwrap())
        .unwrap();
    // A structured session that has already reported the conversation it owns.
    let source = f
        .state
        .store
        .create_session_with_configuration(
            workspace.id,
            ProviderKind::ClaudeCode,
            "Chat session",
            None,
            None,
            None,
            Default::default(),
            false,
        )
        .unwrap();
    f.state
        .store
        .set_interaction_mode(source.id, InteractionMode::Structured)
        .unwrap();
    f.state
        .store
        .set_native_conversation_id(source.id, "11111111-2222-3333-4444-555555555555")
        .unwrap();
    let (status, terminal) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{}/terminal", source.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(terminal["resume_source_id"], source.id.to_string());
    assert_eq!(
        terminal["ephemeral"], true,
        "closing the escape hatch leaves nothing behind; its conversation lives in the session it reopened"
    );
    assert_eq!(
        terminal["status"], "stopped",
        "creating a session launches nothing; the caller starts it through the ordinary route"
    );
    assert_eq!(
        terminal["provider_session_id"],
        "11111111-2222-3333-4444-555555555555"
    );
    assert_eq!(terminal["provider"], "claude_code");
    assert_eq!(terminal["interaction_mode"], "pty");
    assert!(
        terminal["title"]
            .as_str()
            .unwrap()
            .starts_with("Chat session"),
        "the reopen is named after the conversation it reopens"
    );
    let record: Session = serde_json::from_value(terminal).unwrap();
    let spec = providers::build(&f.state, &record, cwd).unwrap();
    assert_eq!(
        without_agentdock(&spec.args),
        vec!["--resume", "11111111-2222-3333-4444-555555555555"]
    );
    // The whole point of the escape hatch: the resume points at the home the
    // structured run wrote into, not one this new session would have created.
    assert_eq!(
        spec.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(
            f.state
                .state_dir
                .join("sessions")
                .join(source.id.to_string())
                .canonicalize()
                .map(|path| dunce::simplified(&path).to_path_buf())
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    assert_ne!(
        spec.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(
            f.state
                .state_dir
                .join("sessions")
                .join(record.id.to_string())
                .to_str()
                .unwrap()
        ),
        "resuming in the new session's own home would find no conversation"
    );
}

#[tokio::test]
async fn opening_a_terminal_before_a_conversation_exists_is_refused() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("fixture", cwd.to_str().unwrap())
        .unwrap();
    let source = f
        .state
        .store
        .create_session_with_configuration(
            workspace.id,
            ProviderKind::Codex,
            "Not started",
            None,
            None,
            None,
            Default::default(),
            false,
        )
        .unwrap();
    f.state
        .store
        .set_interaction_mode(source.id, InteractionMode::Structured)
        .unwrap();
    let (status, body) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{}/terminal", source.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("no conversation"));
}

#[tokio::test]
async fn a_permission_chosen_while_stopped_waits_for_the_next_start() {
    // A session whose client exited -- after a failed turn, say -- still shows
    // the permission chip. Refusing the choice there left the phone with an
    // error hidden behind its settings sheet and nothing changed.
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("fixture", cwd.to_str().unwrap())
        .unwrap();
    let session = f
        .state
        .store
        .create_session_with_configuration(
            workspace.id,
            ProviderKind::ClaudeCode,
            "Stopped",
            None,
            None,
            None,
            Default::default(),
            false,
        )
        .unwrap();
    let path = format!("/api/sessions/{}/conversation/permission", session.id);
    let (status, body) = call(f.app(), "POST", &path, json!({"mode": "danger"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["pending"], true);
    let (status, _) = call(f.app(), "POST", &path, json!({"mode": "everything"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[cfg(unix)]
#[test]
fn native_history_resume_rejects_original_source_retargeted_by_symlink() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("fixture", cwd.to_str().unwrap())
        .unwrap();
    let (source_id, config) = f.native_source(ProviderKind::Codex, Vec::new());
    let session = f
        .state
        .store
        .import_native_session(
            workspace.id,
            ProviderKind::Codex,
            "Original history",
            &source_id,
            "native-id",
            config.to_str().unwrap(),
        )
        .unwrap();
    let original_moved = f.path.join("preserved-original-config");
    fs::rename(&config, &original_moved).unwrap();
    symlink(&original_moved, &config).unwrap();
    let rejection = native_history::resume_spec(&f.state, &session, cwd).unwrap_err();
    assert_eq!(rejection.status, StatusCode::BAD_REQUEST);
    assert!(rejection.message.contains("source changed"));
}

#[tokio::test]
async fn native_config_import_requires_confirmation_and_only_exposes_source_metadata() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (source_id, config) = f.native_source(ProviderKind::Codex, Vec::new());
    let sentinel = b"fake private credential: never read or copy this fixture";
    fs::write(config.join("auth.json"), sentinel).unwrap();
    let (status, sources) = call(
        f.app(),
        "GET",
        "/api/host/native-configurations",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sources[0]["id"], source_id);
    assert_eq!(sources[0]["available"], true);
    assert!(!sources.to_string().contains("private credential"));
    assert!(
        sources[0].get("authenticated").is_none(),
        "Directory presence is not proof of login"
    );
    for body in [
        json!({"source_id":source_id}),
        json!({"source_id":source_id,"confirmed_shared_config":false}),
    ] {
        assert_eq!(
            call(
                f.app(),
                "POST",
                "/api/endpoint-profiles/import-native",
                body
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            json!({"source_id":"../unknown","confirmed_shared_config":true})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            json!({"source_id":source_id,"confirmed_shared_config":true,"config_dir":"/arbitrary"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(call(f.app(), "POST", "/api/endpoint-profiles", json!({"name":"forged","provider":"codex","native_config":{"source_id":source_id,"config_dir":config}})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(f.state.store.list_endpoint_profiles().unwrap().is_empty());
    assert!(!config.join("bridge-used").exists());
    assert_eq!(fs::read(config.join("auth.json")).unwrap(), sentinel);
}

#[tokio::test]
async fn imported_native_profiles_create_new_sessions_without_copying_or_overriding_config() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    for (provider, key, config_file) in [
        (ProviderKind::Codex, "CODEX_HOME", "config.toml"),
        (
            ProviderKind::ClaudeCode,
            "CLAUDE_CONFIG_DIR",
            "settings.json",
        ),
    ] {
        let (source_id, config) = f.native_source(provider.clone(), Vec::new());
        let original = b"Fixture config is intentionally not parsed: preserve these exact bytes";
        fs::write(config.join(config_file), original).unwrap();
        #[cfg(unix)]
        let mode_before = fs::metadata(&config).unwrap().permissions();
        let body =
            json!({"source_id":source_id,"name":"Host account","confirmed_shared_config":true});
        let (status, p) = call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(p["native_config"]["source_id"], source_id);
        assert_eq!(p["native_config"]["config_dir"], config.to_str().unwrap());
        assert_eq!(p["provider"], serde_json::to_value(&provider).unwrap());
        assert_eq!(p["permission_mode"], "native");
        let (_, again) = call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            body,
        )
        .await;
        assert_eq!(again["id"], p["id"]);
        for title in ["First", "Second"] {
            let (status, result) = call(
                f.app(),
                "POST",
                &format!("/api/workspaces/{workspace}/sessions"),
                json!({"provider":provider,"title":title,"endpoint_profile_id":p["id"]}),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            let session: Session = serde_json::from_value(result).unwrap();
            assert!(
                session.provider_session_id.is_none(),
                "This opens a new conversation, not resume"
            );
            assert!(
                session.native_source_id.is_none(),
                "History imports keep their separate contract"
            );
            assert!(matches!(session.status, SessionStatus::Stopped));
            let spec = providers::build(&f.state, &session, cwd.clone()).unwrap();
            assert!(
                without_agentdock(&spec.args).is_empty(),
                "Do not inject login, endpoint, model, permission or resume flags"
            );
            assert_eq!(spec.cwd, cwd);
            // Only the configuration directory, besides the session's own
            // AgentDock identity (agent.rs), which every launch carries.
            let own: Vec<_> = spec
                .env
                .keys()
                .filter(|name| !name.starts_with("AGENTDOCK_"))
                .collect();
            assert_eq!(own, [key]);
            assert_eq!(spec.env[key], config.to_str().unwrap());
            assert!(f.state.runtime.get(&session.id.to_string()).is_none());
            assert!(
                !f.state
                    .state_dir
                    .join("sessions")
                    .join(session.id.to_string())
                    .exists()
            );
        }
        assert!(!config.join("bridge-used").exists());
        assert_eq!(fs::read(config.join(config_file)).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode(),
                mode_before.mode()
            );
        }
    }
    assert_eq!(f.state.store.list_endpoint_profiles().unwrap().len(), 2);
}

#[tokio::test]
async fn native_profile_rename_and_unlink_preserve_existing_session_reference() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (source_id, config) = f.native_source(ProviderKind::Codex, Vec::new());
    let (_, p) = call(
        f.app(),
        "POST",
        "/api/endpoint-profiles/import-native",
        json!({"source_id":source_id,"name":"Original","confirmed_shared_config":true}),
    )
    .await;
    let profile_path = format!("/api/endpoint-profiles/{}", p["id"].as_str().unwrap());
    let create_path = format!("/api/workspaces/{workspace}/sessions");
    // A session may pick its own model even on a shared native configuration:
    // that is a launch flag for this session, and the imported profile below is
    // still asserted to keep the client's own settings.
    let (status, overridden) = call(f.app(), "POST", &create_path, json!({"title":"override","provider":"codex","endpoint_profile_id":p["id"],"model":"other"})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(overridden["endpoint_snapshot"]["model"], "other");
    assert_eq!(
        call(f.app(), "GET", &profile_path, Value::Null).await.1["model"],
        Value::Null
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            &create_path,
            json!({"title":"wrong client","provider":"claude_code","endpoint_profile_id":p["id"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for patch in [
        json!({"endpoint_url":"https://example.test"}),
        json!({"model":null}),
        json!({"proxy_url":null}),
        json!({"permission_mode":"trusted"}),
        json!({"native_config":null}),
        json!({"model_aliases":{}}),
    ] {
        assert_eq!(
            call(f.app(), "PATCH", &profile_path, patch).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (_, record) = call(
        f.app(),
        "POST",
        &create_path,
        json!({"title":"unchanged reference","provider":"codex","endpoint_profile_id":p["id"]}),
    )
    .await;
    let session: Session = serde_json::from_value(record).unwrap();
    let (status, renamed) = call(f.app(), "PATCH", &profile_path, json!({"name":"Renamed"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Renamed");
    let stored = f.state.store.get_session(session.id).unwrap().unwrap();
    assert_eq!(stored.endpoint_snapshot.as_ref().unwrap().name, "Original");
    assert_eq!(
        call(f.app(), "DELETE", &profile_path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    let stored = f.state.store.get_session(session.id).unwrap().unwrap();
    assert!(
        f.state
            .store
            .get_endpoint_profile(session.endpoint_profile_id.unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        stored
            .endpoint_snapshot
            .as_ref()
            .unwrap()
            .native_config
            .is_some()
    );
    let spec = providers::build(
        &f.state,
        &stored,
        dunce::canonicalize(f.path.join("repo")).unwrap(),
    )
    .unwrap();
    assert_eq!(spec.env["CODEX_HOME"], config.to_str().unwrap());
    assert!(
        config.join("history-fixture.json").exists(),
        "Unlink must not delete native history/config"
    );
}

#[tokio::test]
async fn native_profile_rejects_missing_changed_or_wrong_provider_sources() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (source_id, config) = f.native_source(ProviderKind::Codex, Vec::new());
    let (_, p) = call(
        f.app(),
        "POST",
        "/api/endpoint-profiles/import-native",
        json!({"source_id":source_id,"confirmed_shared_config":true}),
    )
    .await;
    let (_, record) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{workspace}/sessions"),
        json!({"title":"pinned","provider":"codex","endpoint_profile_id":p["id"]}),
    )
    .await;
    let session: Session = serde_json::from_value(record).unwrap();
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let replacement = f.path.join("other-native-account");
    fs::create_dir(&replacement).unwrap();
    f.state.native_sources[0].config_dir = replacement;
    assert_eq!(
        providers::build(&f.state, &session, cwd.clone())
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            &format!("/api/workspaces/{workspace}/sessions"),
            json!({"title":"changed source","provider":"codex","endpoint_profile_id":p["id"]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    f.state.native_sources[0].config_dir = config.clone();
    f.state.native_sources[0].provider = ProviderKind::ClaudeCode;
    assert_eq!(
        providers::build(&f.state, &session, cwd.clone())
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    f.state.native_sources[0].provider = ProviderKind::Codex;
    f.state.native_sources[0].config_dir = f.path.join("missing-native-source");
    assert_eq!(
        providers::build(&f.state, &session, cwd.clone())
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            json!({"source_id":source_id,"confirmed_shared_config":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    f.state.native_sources.clear();
    assert_eq!(
        providers::build(&f.state, &session, cwd)
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(config.join("history-fixture.json").exists());
}

#[tokio::test]
async fn native_configuration_sources_are_not_public_without_authentication() {
    let f = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("a-long-enough-fixture-token-12345"),
    );
    assert_eq!(
        call(
            f.app(),
            "GET",
            "/api/host/native-configurations",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            f.app(),
            "POST",
            "/api/endpoint-profiles/import-native",
            json!({"source_id":"codex-default","confirmed_shared_config":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn native_config_keeps_default_and_explicit_login_contexts_distinct() {
    for (provider, key) in [
        (ProviderKind::ClaudeCode, "CLAUDE_CONFIG_DIR"),
        (ProviderKind::Codex, "CODEX_HOME"),
    ] {
        let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
        let (source_id, config) = f.native_source(provider.clone(), Vec::new());
        let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
        f.state.native_sources[0].config_env = None;
        let (_, reference) = native_config::pin(&f.state, &source_id).unwrap();
        assert!(reference.config_env.is_none());
        let spec = native_config::build(&f.state, &provider, &reference, cwd.clone()).unwrap();
        assert!(
            !spec.env.contains_key(key),
            "Do not replace an unset default home with an explicit same-path env value"
        );
        assert!(spec.env_remove.iter().any(|removed| removed == key));
        assert!(
            native_config::source_views(&f.state)[0]
                .config_env
                .is_none()
        );
        f.state.native_sources[0].config_env = Some(config.as_os_str().to_owned());
        assert_eq!(
            native_config::validate_reference(&f.state, &provider, &reference)
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        let (_, explicit) = native_config::pin(&f.state, &source_id).unwrap();
        let spec = native_config::build(&f.state, &provider, &explicit, cwd).unwrap();
        assert_eq!(spec.env[key], config.to_str().unwrap());
        assert!(!spec.env_remove.iter().any(|removed| removed == key));
        f.state.native_sources[0].config_env = Some("relative-native-home".into());
        assert!(
            native_config::pin(&f.state, &source_id)
                .unwrap_err()
                .message
                .contains("relative")
        );
    }
}

fn canvas_document(panes: Vec<Value>) -> Value {
    json!({"version":1,"root":{"type":"stack","kind":"stack","id":"shared-root","panes":panes}})
}

#[tokio::test]
async fn session_environment_api_merges_templates_and_only_edits_stopped_sessions() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (_,profile) = call(f.app(),"POST","/api/endpoint-profiles",json!({"name":"env","provider":"codex","environment":{"MODE":{"kind":"literal","value":"template"},"KEEP":{"kind":"literal","value":"keep"}}})).await;
    let (status,session) = call(f.app(),"POST",&format!("/api/workspaces/{workspace}/sessions"),json!({"title":"env","provider":"codex","endpoint_profile_id":profile["id"],"environment":{"MODE":{"kind":"unset"}}})).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    assert_eq!(session["environment"]["MODE"]["kind"], "unset");
    assert_eq!(session["environment"]["KEEP"]["value"], "keep");
    assert_eq!(
        session["endpoint_snapshot"]["environment"]["MODE"]["value"],
        "template"
    );
    let id: Uuid = session["id"].as_str().unwrap().parse().unwrap();
    let url = format!("/api/sessions/{id}/environment");
    let changes = json!({"environment":{"OPENAI_API_KEY":{"kind":"secret_ref","reference":"env:AGENTDOCK_SECRET_FIXTURE"}}});
    let (status, patched) = call(f.app(), "PATCH", &url, changes.clone()).await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["environment"], changes["environment"]);
    assert_eq!(patched["endpoint_snapshot"], session["endpoint_snapshot"]);
    f.state
        .store
        .set_session_status(id, SessionStatus::Running)
        .unwrap();
    assert_eq!(
        call(f.app(), "PATCH", &url, json!({"environment":{}}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    f.state
        .store
        .set_session_status(id, SessionStatus::Failed)
        .unwrap();
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &url,
            json!({"environment":{"HOME":{"kind":"unset"}}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &url,
            json!({"environment":{"API_KEY":{"kind":"literal","value":"synthetic-no"}}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(f.app(), "PATCH", &url, json!({"environment":{}}))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn session_environment_rejects_live_runtime_even_if_database_says_stopped() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = f
        .state
        .store
        .create_workspace("fixture", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let session = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Terminal, "fixture shell")
        .unwrap();
    let runtime = f
        .state
        .runtime
        .start(
            session.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: f.path.join("repo"),
                env: Default::default(),
                env_remove: vec![],
            },
        )
        .await
        .unwrap();
    let status = call(
        f.app(),
        "PATCH",
        &format!("/api/sessions/{}/environment", session.id),
        json!({"environment":{}}),
    )
    .await
    .0;
    runtime.stop().await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn native_and_history_build_apply_environment_without_mutating_native_configuration() {
    use agentdock_domain::{EnvironmentOverrides, EnvironmentValue};
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (source_id, config) = f.native_source(ProviderKind::Codex, Vec::new());
    let sentinel = config.join("config.toml");
    fs::write(&sentinel, "# immutable fixture\n").unwrap();
    let workspace = f
        .state
        .store
        .create_workspace("fixture", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let (_, reference) = native_config::pin(&f.state, &source_id).unwrap();
    let mut profile = f
        .state
        .store
        .import_native_profile("native", ProviderKind::Codex, reference)
        .unwrap();
    profile.environment = [(
        "NATIVE_TEST_MODE".into(),
        EnvironmentValue::Literal {
            value: "template".into(),
        },
    )]
    .into();
    f.state.store.update_endpoint_profile(&profile).unwrap();
    let mut session = f
        .state
        .store
        .create_session_with_profile(
            workspace.id,
            ProviderKind::Codex,
            "native",
            Some(profile.id),
        )
        .unwrap();
    let spec = providers::build(&f.state, &session, f.path.join("repo")).unwrap();
    assert_eq!(spec.env["NATIVE_TEST_MODE"], "template");
    assert_eq!(spec.env["CODEX_HOME"], config.to_str().unwrap());
    let environment: EnvironmentOverrides = [
        (
            "NATIVE_TEST_MODE".into(),
            EnvironmentValue::Literal {
                value: "history".into(),
            },
        ),
        ("REMOVE_ME".into(), EnvironmentValue::Unset),
    ]
    .into();
    session = f
        .state
        .store
        .import_native_session_with_environment(
            workspace.id,
            ProviderKind::Codex,
            "history",
            &source_id,
            "fixture-thread",
            config.to_str().unwrap(),
            Some(environment),
        )
        .unwrap()
        .unwrap();
    let spec = providers::build(&f.state, &session, f.path.join("repo")).unwrap();
    assert_eq!(spec.env["NATIVE_TEST_MODE"], "history");
    assert!(spec.env_remove.contains(&"REMOVE_ME".into()));
    assert!(!spec.env_remove.contains(&"PATH".into()));
    assert!(!spec.env.contains_key("PATH"));
    assert_eq!(
        fs::read_to_string(sentinel).unwrap(),
        "# immutable fixture\n"
    );
}

#[tokio::test]
async fn history_import_environment_conflicts_without_overwriting_existing_session() {
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let cwd = dunce::canonicalize(f.path.join("repo")).unwrap();
    let (source_id,_) = f.native_source(ProviderKind::Codex,vec![json!({"id":"fixture-thread","provider":"codex","cwd":cwd,"title":"fixture","updated_at":"2026-09-09T00:00:00Z"})]);
    let url = format!("/api/workspaces/{workspace}/native-history/import");
    let input = json!({"source_id":source_id,"native_id":"fixture-thread","confirmed_original_config":true,"environment":{"MODE":{"kind":"literal","value":"first"}}});
    let (status, session) = call(f.app(), "POST", &url, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    f.state
        .store
        .set_session_status(
            session["id"].as_str().unwrap().parse().unwrap(),
            SessionStatus::Running,
        )
        .unwrap();
    let mut changed = input.clone();
    changed["environment"] = json!({});
    assert_eq!(
        call(f.app(), "POST", &url, changed).await.0,
        StatusCode::CONFLICT
    );
    let mut no_override = input;
    no_override.as_object_mut().unwrap().remove("environment");
    let (_, unchanged) = call(f.app(), "POST", &url, no_override).await;
    assert_eq!(unchanged["environment"], session["environment"]);
    assert_eq!(unchanged["status"], "running");
}

#[tokio::test]
async fn session_environment_api_requires_existing_authentication() {
    let f = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("fixture-token-long-enough"),
    );
    assert_eq!(
        call(
            f.app(),
            "PATCH",
            &format!("/api/sessions/{}/environment", Uuid::new_v4()),
            json!({"environment":{}})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn shared_canvas_roundtrips_multiple_projects_without_migrating_legacy_layouts() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let first_workspace = register(&f).await;
    let first_id = first_workspace.parse().unwrap();
    // An unavailable project directory is still valid layout ownership. Saving
    // a canvas must not read/stat project files or initialize client credentials.
    let other_path = f.path.join("unavailable-project");
    let second_workspace = f
        .state
        .store
        .create_workspace("Second project", other_path.to_str().unwrap())
        .unwrap();
    let first_session = f
        .state
        .store
        .create_session(first_id, ProviderKind::Codex, "First agent")
        .unwrap();
    let second_session = f
        .state
        .store
        .create_session(
            second_workspace.id,
            ProviderKind::ClaudeCode,
            "Second agent",
        )
        .unwrap();
    let legacy = default_layout();
    f.state
        .store
        .save_layout(first_id, &legacy.to_string())
        .unwrap();
    f.state
        .store
        .save_layout(second_workspace.id, "unparsed legacy layout")
        .unwrap();
    let (status, initial) = call(f.app(), "GET", "/api/canvas/layout", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial, json!({"layout":null,"revision":0}));
    let layout = json!({"version":1,"root":{
        "type":"split","id":"multi-project","direction":"horizontal","ratio":0.5,
        "first":{"type":"stack","kind":"stack","id":"first-stack","panes":[
            {"type":"pane","id":"first-agent","kind":"agent_chat","metadata":{"workspace_id":first_workspace,"session_id":first_session.id,"provider":"codex"}},
            {"type":"pane","id":"first-editor","kind":"editor","metadata":{"workspace_id":first_workspace,"path":"src/not-read.rs"}}
        ]},
        "second":{"type":"stack","kind":"stack","id":"second-stack","panes":[
            {"type":"pane","id":"second-agent","kind":"agent_chat","metadata":{"workspace_id":second_workspace.id,"session_id":second_session.id,"provider":"claude_code"}},
            {"type":"pane","id":"second-diff","kind":"git_diff","metadata":{"workspace_id":second_workspace.id}},
            {"type":"pane","id":"second-preview","kind":"file_preview","metadata":{"workspace_id":second_workspace.id,"path":"images/not-read.png"}}
        ]}
    }});
    let (status, saved) = call(
        f.app(),
        "PUT",
        "/api/canvas/layout",
        json!({"layout":layout,"expected_revision":0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved, json!({"revision":1}));
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .1,
        json!({"layout":layout,"revision":1})
    );
    let mut changed = layout.clone();
    changed["root"]["ratio"] = json!(0.4);
    assert_eq!(
        call(
            f.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":changed,"expected_revision":0})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .1,
        json!({"layout":layout,"revision":1})
    );
    assert_eq!(
        call(
            f.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":changed,"expected_revision":1})
        )
        .await
        .1,
        json!({"revision":2})
    );
    assert_eq!(
        call(
            f.app(),
            "GET",
            &format!("/api/workspaces/{first_workspace}/layout"),
            Value::Null
        )
        .await
        .1,
        legacy
    );
    assert_eq!(
        f.state
            .store
            .get_layout(second_workspace.id)
            .unwrap()
            .as_deref(),
        Some("unparsed legacy layout")
    );
    assert!(matches!(
        f.state
            .store
            .get_session(first_session.id)
            .unwrap()
            .unwrap()
            .status,
        SessionStatus::Stopped
    ));
    assert!(f.state.runtime.get(&first_session.id.to_string()).is_none());
    assert!(!other_path.exists());
}

#[tokio::test]
async fn shared_canvas_rejects_unknown_owners_wrong_session_bindings_and_unsafe_paths() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let second = f
        .state
        .store
        .create_workspace("Other", "/fixture/other")
        .unwrap();
    let session = f
        .state
        .store
        .create_session(workspace.parse().unwrap(), ProviderKind::Codex, "bound")
        .unwrap();
    let cases = [
        (json!({"kind":"git_diff"}), StatusCode::BAD_REQUEST),
        (json!({"kind":"file_preview"}), StatusCode::BAD_REQUEST),
        (
            json!({"kind":"editor","metadata":{"path":"src/main.rs"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"session_id":session.id}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":"bad-id"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":null}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":Uuid::new_v4()}}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"kind":"editor","metadata":[]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":workspace,"session_id":"bad-session"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":workspace,"session_id":Uuid::new_v4()}}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":second.id,"session_id":session.id}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"session_id":session.id}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"agent_chat","metadata":{"workspace_id":workspace,"session_id":session.id,"provider":"claude_code"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"terminal","metadata":{"workspace_id":workspace,"session_id":session.id,"provider":"codex"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"terminal","metadata":{"workspace_id":workspace,"session_id":session.id,"provider":false}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"path":"/outside/file"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"path":"../outside"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"path":"nested/../../outside"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"path":"nul\u{0}name"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"editor","metadata":{"workspace_id":workspace,"path":27}}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"kind":"unknown-pane"}), StatusCode::BAD_REQUEST),
    ];
    for (mut pane, expected) in cases {
        pane["type"] = json!("pane");
        pane["id"] = json!("invalid-pane");
        let (status, response) = call(
            f.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":canvas_document(vec![pane.clone()]),"expected_revision":0}),
        )
        .await;
        assert_eq!(status, expected, "pane={pane}, response={response}");
    }
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .1,
        json!({"layout":null,"revision":0})
    );
}

#[tokio::test]
async fn shared_canvas_supports_unbound_placeholders_but_enforces_layout_limits() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let placeholders = canvas_document(vec![
        json!({"type":"pane","id":"agent","kind":"agent_chat"}),
        json!({"type":"pane","id":"editor","kind":"editor"}),
        json!({"type":"pane","id":"terminal","kind":"terminal"}),
    ]);
    assert_eq!(
        call(
            f.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":placeholders,"expected_revision":0})
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut collapsed = placeholders.clone();
    collapsed["collapsed"] = json!([{"node":{"type":"pane","id":"hidden","kind":"git_diff"}}]);
    let mut oversized = placeholders.clone();
    oversized["root"]["panes"][0]["title"] = json!("x".repeat(256 * 1024));
    let mut duplicate = placeholders.clone();
    duplicate["root"]["panes"][1]["id"] = json!("agent");
    let many = canvas_document(
        (0..513)
            .map(|index| json!({"type":"pane","id":format!("pane-{index}"),"kind":"editor"}))
            .collect(),
    );
    let mut deep = json!({"type":"pane","id":"deep-leaf","kind":"editor"});
    for depth in 0..34 {
        deep = json!({"type":"split","id":format!("split-{depth}"),"direction":"horizontal","ratio":0.5,"first":deep,"second":{"type":"pane","id":format!("other-{depth}"),"kind":"editor"}});
    }
    for layout in [
        collapsed,
        oversized,
        duplicate,
        many,
        json!({"version":1,"root":deep}),
        Value::Null,
    ] {
        assert_eq!(
            call(
                f.app(),
                "PUT",
                "/api/canvas/layout",
                json!({"layout":layout,"expected_revision":1})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for revision in [json!(-1), json!(0.5), json!("1")] {
        assert_eq!(
            call(
                f.app(),
                "PUT",
                "/api/canvas/layout",
                json!({"layout":placeholders,"expected_revision":revision})
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .1,
        json!({"layout":placeholders,"revision":1})
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shared_canvas_keeps_legal_unix_backslashes_in_relative_filenames() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let layout = canvas_document(vec![
        json!({"type":"pane","id":"backslashes","kind":"editor","metadata":{"workspace_id":workspace,"path":"literal\\filename.txt"}}),
    ]);
    let (status, response) = call(
        f.app(),
        "PUT",
        "/api/canvas/layout",
        json!({"layout":layout,"expected_revision":0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .1,
        json!({"layout":layout,"revision":1})
    );
}

#[tokio::test]
async fn shared_canvas_uses_existing_authentication_and_health_capabilities() {
    let f = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("fixture-token-long-enough"),
    );
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            f.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":canvas_document(Vec::new()),"expected_revision":0})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for (method, path) in [
        ("GET", "/api/secrets"),
        ("PUT", "/api/secrets/AGENTDOCK_SECRET_X"),
        ("GET", "/api/update"),
        ("POST", "/api/update"),
        ("POST", "/api/update/restart"),
    ] {
        assert_eq!(
            call(f.app(), method, path, json!({"value":"x"})).await.0,
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
    let (status, health) = call(f.app(), "GET", "/api/health", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["api_version"], 2);
    assert_eq!(
        health["capabilities"],
        json!([
            "shared_canvas",
            "native_configurations",
            "native_history",
            "host_directories",
            "endpoint_models",
            "session_environment",
            "structured_chat",
            "official_accounts",
            "session_configuration",
            "session_archive",
            "account_import_native",
            "agent_clients",
            "ephemeral_sessions",
            "session_model",
            "workspace_file_search",
            "session_terminal_escape",
            "stored_secrets",
            "agent_tools",
            "self_update",
            "resource_history"
        ])
    );
    let allowed = Request::builder()
        .uri("/api/canvas/layout")
        .header("host", "127.0.0.1:8787")
        .header("authorization", "Bearer fixture-token-long-enough")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.app().oneshot(allowed).await.unwrap().status(),
        StatusCode::OK
    );
    let attack = Request::builder()
        .method("PUT")
        .uri("/api/canvas/layout")
        .header("host", "127.0.0.1:8787")
        .header("authorization", "Bearer fixture-token-long-enough")
        .header("x-agentdock-client", "web")
        .header("origin", "https://attacker.invalid")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"layout":canvas_document(Vec::new()),"expected_revision":0}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        f.app().oneshot(attack).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let missing_client_header = Request::builder()
        .method("PUT")
        .uri("/api/canvas/layout")
        .header("host", "127.0.0.1:8787")
        .header("authorization", "Bearer fixture-token-long-enough")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"layout":canvas_document(Vec::new()),"expected_revision":0}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        f.app()
            .oneshot(missing_client_header)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.state.store.get_shared_canvas_layout().unwrap().revision,
        0
    );
}

#[tokio::test]
async fn shared_canvas_corrupt_storage_never_silently_falls_back_to_a_legacy_layout() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    f.state
        .store
        .save_shared_canvas_layout("invalid json", 0)
        .unwrap();
    assert_eq!(
        call(f.app(), "GET", "/api/canvas/layout", Value::Null)
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        f.state
            .store
            .get_shared_canvas_layout()
            .unwrap()
            .layout
            .as_deref(),
        Some("invalid json")
    );
}

#[cfg(unix)]
#[test]
fn native_config_preserves_explicit_directory_spelling_and_rejects_symlink_retarget() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (source_id, config) = f.native_source(ProviderKind::ClaudeCode, Vec::new());
    let alias = f.path.join("original-native-alias");
    symlink(&config, &alias).unwrap();
    f.state.native_sources[0].config_dir = alias.clone();
    f.state.native_sources[0].config_env = Some(alias.as_os_str().to_owned());
    let (_, reference) = native_config::pin(&f.state, &source_id).unwrap();
    assert_eq!(reference.config_dir, config.to_str().unwrap());
    assert_eq!(reference.config_env.as_deref(), alias.to_str());
    let views = native_config::source_views(&f.state);
    assert_eq!(views[0].path, reference.config_dir);
    assert_eq!(views[0].config_env, reference.config_env);
    let spec = native_config::build(
        &f.state,
        &ProviderKind::ClaudeCode,
        &reference,
        dunce::canonicalize(f.path.join("repo")).unwrap(),
    )
    .unwrap();
    assert_eq!(spec.env["CLAUDE_CONFIG_DIR"], alias.to_str().unwrap());
    let replacement = f.path.join("another-account");
    fs::create_dir(&replacement).unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(replacement, alias).unwrap();
    assert_eq!(
        native_config::validate_reference(&f.state, &ProviderKind::ClaudeCode, &reference)
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
}

/// Usage capture rides along with the launch instead of editing settings files
/// the user owns, and it must not silently replace a status line they already
/// configured.
#[tokio::test]
async fn claude_usage_capture_is_a_launch_overlay_that_preserves_an_existing_status_line() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let home = dunce::canonicalize(&f.path).unwrap().join("claude-home");
    fs::create_dir_all(&home).unwrap();
    let config = home.to_str().unwrap();

    // Off by default: a session launches exactly as the native client would.
    assert!(!native_config::usage_capture_enabled());
    assert_eq!(
        native_config::claude_statusline(&f.state, Some(config)),
        (Vec::new(), std::collections::BTreeMap::new())
    );

    // SAFETY: single-threaded assertion of this process-wide opt-in.
    unsafe { std::env::set_var("AGENTDOCK_CLAUDE_USAGE_CAPTURE", "1") };
    let (args, environment) = native_config::claude_statusline(&f.state, Some(config));
    assert_eq!(args[0], "--settings");
    let overlay: Value = serde_json::from_str(&args[1]).unwrap();
    let command = overlay["statusLine"]["command"].as_str().unwrap();
    assert!(command.contains("statusline-capture.mjs"));
    assert_eq!(overlay["statusLine"]["type"], "command");
    // The overlay is the only key, so nothing else about the session's settings
    // is asserted or overridden by AgentDock.
    assert_eq!(overlay.as_object().unwrap().len(), 1);
    // No status line of their own yet, so there is nothing to delegate to.
    assert!(!environment.contains_key("AGENTDOCK_STATUSLINE_DELEGATE"));
    // No settings file belonging to the account is created or modified.
    assert!(!home.join("settings.json").exists());

    fs::write(
        home.join("settings.json"),
        r#"{"statusLine":{"type":"command","command":"my-own-status-line"},"model":"keep-me"}"#,
    )
    .unwrap();
    let (_, environment) = native_config::claude_statusline(&f.state, Some(config));
    assert_eq!(
        environment["AGENTDOCK_STATUSLINE_DELEGATE"],
        "my-own-status-line"
    );
    let untouched = fs::read_to_string(home.join("settings.json")).unwrap();
    assert!(untouched.contains("my-own-status-line") && untouched.contains("keep-me"));

    // A capture command already in place must not be delegated back into itself.
    fs::write(
        home.join("settings.json"),
        format!(
            r#"{{"statusLine":{{"type":"command","command":{}}}}}"#,
            Value::from(command)
        ),
    )
    .unwrap();
    let (_, environment) = native_config::claude_statusline(&f.state, Some(config));
    assert!(!environment.contains_key("AGENTDOCK_STATUSLINE_DELEGATE"));

    for broken in [
        "not json",
        r#"{"statusLine":{"type":"command"}}"#,
        r#"{"statusLine":{"type":"command","command":"   "}}"#,
    ] {
        fs::write(home.join("settings.json"), broken).unwrap();
        let (args, environment) = native_config::claude_statusline(&f.state, Some(config));
        assert_eq!(args[0], "--settings", "{broken}");
        assert!(
            !environment.contains_key("AGENTDOCK_STATUSLINE_DELEGATE"),
            "{broken}"
        );
    }
    // SAFETY: restores the default for every other test in this process.
    unsafe { std::env::remove_var("AGENTDOCK_CLAUDE_USAGE_CAPTURE") };
}

/// Client discovery is read-only, and installing is an explicit action that
/// downloads and runs package code.
#[tokio::test]
async fn client_discovery_reports_this_host_and_refuses_an_unconfirmed_install() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (status, clients) = call(f.app(), "GET", "/api/clients", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let clients = clients.as_array().unwrap();
    // Terminal is a shell, not an installable agent client.
    assert_eq!(clients.len(), 2);
    for client in clients {
        let provider = client["provider"].as_str().unwrap();
        assert!(matches!(provider, "claude_code" | "codex"));
        assert_eq!(
            client["npm_package"],
            if provider == "codex" {
                "@openai/codex"
            } else {
                "@anthropic-ai/claude-code"
            }
        );
        // An unknown client is never reported as installed, and a program name
        // is always present so a failed launch keeps its own error.
        assert!(
            client["program"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        if client["source"] == "missing" {
            assert_eq!(client["installed"], false);
            assert!(client["version"].is_null());
        }
        // Nothing is installed by looking.
        assert!(!crate::clients::managed_root(&f.state.state_dir).exists());
    }
    for (provider, body, expected) in [
        (
            "codex",
            json!({"confirmed": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "terminal",
            json!({"confirmed": true}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "nonsense",
            json!({"confirmed": true}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = call(
            f.app(),
            "POST",
            &format!("/api/clients/{provider}/install"),
            body,
        )
        .await;
        assert_eq!(status, expected, "{provider}");
    }
    assert!(!crate::clients::managed_root(&f.state.state_dir).exists());
}

/// Attachments land inside the workspace under their own directory, keep a
/// recognisable name, and cannot be steered by a client-supplied path.
#[tokio::test]
async fn attachment_upload_stays_inside_its_own_workspace_directory() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let upload = |name: &str, body: &'static [u8]| {
        let app = f.app();
        let uri = format!(
            "/api/workspaces/{id}/attachments?name={}",
            urlencoding(name)
        );
        async move {
            let request = Request::builder()
                .method("POST")
                .uri(uri)
                .header("host", "127.0.0.1:8787")
                .header("x-agentdock-client", "web")
                .header("content-type", "application/octet-stream")
                .body(Body::from(body))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let (status, saved) = upload("my holiday photo.JPG", b"fixture-bytes").await;
    assert_eq!(status, StatusCode::OK);
    let path = saved["path"].as_str().unwrap();
    assert!(path.starts_with(".agentdock-files/"), "{path}");
    assert!(path.ends_with(".jpg"), "{path}");
    // The stored path is handed to an agent as text, so it carries no spaces.
    assert!(!path.contains(' '), "{path}");
    assert!(path.contains("my-holiday-photo"), "{path}");
    // The label keeps the user's own spelling.
    assert_eq!(saved["name"], "my holiday photo.jpg");
    assert_eq!(saved["bytes"], 13);
    let stored = f.path.join("repo").join(path);
    assert_eq!(fs::read(&stored).unwrap(), b"fixture-bytes");
    // Uploads must not become pending Git changes in the user's repository,
    // and the ignore stays scoped to this directory.
    assert_eq!(
        fs::read_to_string(f.path.join("repo/.agentdock-files/.gitignore")).unwrap(),
        "*\n"
    );
    assert!(!f.path.join("repo/.gitignore").exists());
    let (_, status) = call(
        f.app(),
        "GET",
        &format!("/api/workspaces/{id}/git/status"),
        Value::Null,
    )
    .await;
    assert!(
        !status["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"]
                .as_str()
                .is_some_and(|path| path.contains(".agentdock-files"))),
        "{status:?}"
    );

    // Two uploads of the same camera name both survive.
    let (_, second) = upload("my holiday photo.JPG", b"second-bytes").await;
    assert_ne!(second["path"], saved["path"]);
    assert_eq!(fs::read(&stored).unwrap(), b"fixture-bytes");

    // A path in the name is reduced to its base name; nothing escapes.
    let (status, traversal) = upload("../../etc/passwd", b"nope").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        traversal["path"]
            .as_str()
            .unwrap()
            .starts_with(".agentdock-files/"),
        "{traversal:?}"
    );
    assert!(!f.path.join("etc").exists());
    assert_eq!(traversal["name"], "passwd");

    // Empty uploads and blank names are refused rather than stored.
    assert_eq!(upload("empty.txt", b"").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(upload("   ", b"data").await.0, StatusCode::BAD_REQUEST);
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[tokio::test]
async fn preferences_are_validated_per_provider_and_kept() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (status, empty) = call(f.app(), "GET", "/api/preferences", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, json!({"claude_code":{},"codex":{}}));
    // Codex has no plan mode, and a depth must be one the clients know.
    for bad in [
        json!({"codex":{"permission":"plan"}}),
        json!({"claude_code":{"effort":"extreme"}}),
        json!({"claude_code":{"endpoint_profile_id":Uuid::new_v4()}}),
    ] {
        assert_ne!(
            call(f.app(), "PUT", "/api/preferences", bad).await.0,
            StatusCode::OK
        );
    }
    let (_, profile) = call(
        f.app(),
        "POST",
        "/api/endpoint-profiles",
        json!({"name":"work","provider":"codex","endpoint_url":"https://example.test/v1","permission_mode":"interactive"}),
    )
    .await;
    let pid = profile["id"].as_str().unwrap();
    // A profile belongs to one provider's preferences only.
    assert_eq!(
        call(
            f.app(),
            "PUT",
            "/api/preferences",
            json!({"claude_code":{"endpoint_profile_id":pid}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let chosen = json!({"claude_code":{"effort":"high","permission":"plan"},"codex":{"endpoint_profile_id":pid,"permission":"ask"}});
    let (status, _) = call(f.app(), "PUT", "/api/preferences", chosen.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(f.app(), "GET", "/api/preferences", Value::Null)
            .await
            .1,
        chosen
    );
    // A new session takes the preferred depth unless it says otherwise; an
    // explicit null is "automatic" and takes none.
    let workspace = register(&f).await;
    let path = format!("/api/workspaces/{workspace}/sessions");
    let (_, defaulted) = call(
        f.app(),
        "POST",
        &path,
        json!({"title":"a","provider":"claude_code"}),
    )
    .await;
    assert_eq!(
        defaulted["endpoint_snapshot"]["effort"], "high",
        "{defaulted}"
    );
    let (_, automatic) = call(
        f.app(),
        "POST",
        &path,
        json!({"title":"b","provider":"claude_code","effort":null}),
    )
    .await;
    assert!(
        automatic["endpoint_snapshot"]["effort"].is_null(),
        "{automatic}"
    );
    let (_, low) = call(
        f.app(),
        "POST",
        &path,
        json!({"title":"c","provider":"claude_code","effort":"low"}),
    )
    .await;
    assert_eq!(low["endpoint_snapshot"]["effort"], "low");
}

#[tokio::test]
async fn steering_is_validated_like_a_message() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = register(&f).await;
    let (_, session) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{workspace}/sessions"),
        json!({"title":"steer","provider":"claude_code","interaction_mode":"structured"}),
    )
    .await;
    let path = format!(
        "/api/sessions/{}/conversation/steer",
        session["id"].as_str().unwrap()
    );
    for bad in [
        json!({"id":"not-a-uuid","content":"hello"}),
        json!({"id":Uuid::new_v4(),"content":"   "}),
    ] {
        assert_eq!(
            call(f.app(), "POST", &path, bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn an_agent_lists_and_closes_canvas_tabs_and_asks_before_discarding_a_working_one() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = f
        .state
        .store
        .create_workspace("tabs", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let kept = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::Terminal, "kept")
        .unwrap();
    let temporary = f
        .state
        .store
        .create_session_with_options(
            workspace.id,
            ProviderKind::Terminal,
            "scratch",
            None,
            None,
            None,
            Default::default(),
            true,
            None,
            None,
        )
        .unwrap();
    let pane = |id: &str, session: &Session| json!({"type":"pane","id":id,"kind":"terminal","title":session.title,"metadata":{"session_id":session.id,"workspace_id":workspace.id}});
    let layout = json!({"version":1,"root":{"type":"split","id":"root","direction":"horizontal","ratio":0.5,
        "first":{"type":"stack","kind":"stack","id":"left","activePaneId":"one","panes":[pane("one", &kept), pane("two", &temporary)]},
        "second":{"type":"pane","id":"changes","kind":"git_diff","title":"Changes","metadata":{"workspace_id":workspace.id}}}});
    let (status, saved) = call(
        f.app(),
        "PUT",
        "/api/canvas/layout",
        json!({"layout":layout,"expected_revision":0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    f.state
        .runtime
        .start(
            temporary.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: f.path.join("repo"),
                env: BTreeMap::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .unwrap();

    let canvas = |arguments: Value| {
        agent_request(
            f.app(),
            "POST",
            "/api/agent/call",
            None,
            Some("agent"),
            json!({"name":"agentdock_canvas","arguments":arguments}),
        )
    };
    let listed = tool_text(&canvas(json!({"action":"list"})).await.1);
    let tabs = listed["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3);
    assert_eq!(tabs[1]["temporary"], true);
    assert_eq!(tabs[1]["running"], true);

    let mut events = f.state.activity.subscribe();
    // A plain tab closes at once; the session behind it is untouched.
    let closed = canvas(json!({"action":"close","pane_ids":["one"]})).await.1;
    assert_eq!(tool_text(&closed)["closed"], 1, "{closed}");
    assert_eq!(next_event(&mut events, "canvas").await["revision"], 2);
    let notice = next_event(&mut events, "notice").await;
    assert!(f.state.store.get_session(kept.id).unwrap().is_some());
    let (_, current) = call(f.app(), "GET", "/api/canvas/layout", Value::Null).await;
    assert_eq!(current["layout"]["root"]["first"]["activePaneId"], "two");
    // Undo puts it back.
    call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/notices/{}/undo",
            notice["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    let (_, current) = call(f.app(), "GET", "/api/canvas/layout", Value::Null).await;
    assert_eq!(
        current["layout"]["root"]["first"]["panes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let unknown = canvas(json!({"action":"close","pane_ids":["nope"]}))
        .await
        .1;
    assert_eq!(unknown["isError"], true);

    // Closing everything takes a working temporary session with it: asked first.
    let app = f.app();
    let all = tokio::spawn(async move {
        agent_request(
            app,
            "POST",
            "/api/agent/call",
            None,
            Some("agent"),
            json!({"name":"agentdock_canvas","arguments":{"action":"close","all":true}}),
        )
        .await
        .1
    });
    let request = next_event(&mut events, "request").await;
    assert_eq!(request["values"]["count"], 3);
    assert_eq!(request["rows"][0]["value"], "scratch");
    call(
        f.app(),
        "POST",
        &format!(
            "/api/agent-activity/requests/{}",
            request["id"].as_str().unwrap()
        ),
        json!({"approve":true}),
    )
    .await;
    let result = tool_text(&all.await.unwrap());
    assert_eq!(result["discarded_temporary_sessions"], 1);
    assert!(f.state.store.get_session(temporary.id).unwrap().is_none());
    let (_, current) = call(f.app(), "GET", "/api/canvas/layout", Value::Null).await;
    assert_eq!(current["layout"]["root"]["panes"], json!([]));
}

#[tokio::test]
async fn a_session_can_have_the_agent_tools_on_or_off_whatever_the_preference() {
    const DEPLOYMENT: &str = "deployment-token-for-session-switch-012345";
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), Some(DEPLOYMENT));
    let workspace = f
        .state
        .store
        .create_workspace("switch", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let claude = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::ClaudeCode, "claude")
        .unwrap();
    let cwd = f.path.join("repo");
    let equipped = |spec: &agentdock_runtime::SpawnSpec| {
        spec.args.iter().any(|arg| arg.starts_with("--mcp-config="))
    };
    let web = |method: &'static str, path: String, body: Value| {
        agent_request(
            f.app(),
            method,
            path.leak(),
            Some(DEPLOYMENT),
            Some("web"),
            body,
        )
    };
    let path = format!("/api/sessions/{}/agent-tools", claude.id);

    let (status, view) = web("GET", path.clone(), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view, json!({"choice":null,"default":true,"enabled":true}));

    // Off for this session, though the preference is on.
    let (_, view) = web("PUT", path.clone(), json!({"choice":false})).await;
    assert_eq!(view["enabled"], false);
    let spec = providers::build(&f.state, &claude, cwd.clone()).unwrap();
    assert!(!equipped(&spec) && !spec.env.contains_key("AGENTDOCK_AGENT_TOKEN"));

    // On for this session, though the preference is off.
    web(
        "PUT",
        "/api/preferences".into(),
        json!({"claude_code":{},"codex":{},"agent_tools":false}),
    )
    .await;
    let (_, view) = web("PUT", path.clone(), json!({"choice":true})).await;
    assert_eq!(view, json!({"choice":true,"default":false,"enabled":true}));
    assert!(equipped(
        &providers::build(&f.state, &claude, cwd.clone()).unwrap()
    ));

    // Back to following the preference.
    let (_, view) = web("PUT", path.clone(), json!({"choice":null})).await;
    assert_eq!(view["enabled"], false);
    assert!(!equipped(
        &providers::build(&f.state, &claude, cwd.clone()).unwrap()
    ));

    let (status, _) = web(
        "PUT",
        format!("/api/sessions/{}/agent-tools", Uuid::new_v4()),
        json!({"choice":true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A session cannot switch its own tools: its token opens /api/agent/ only.
    web("PUT", path.clone(), json!({"choice":true})).await;
    let token = providers::build(&f.state, &claude, cwd.clone())
        .unwrap()
        .env["AGENTDOCK_AGENT_TOKEN"]
        .clone();
    f.state
        .runtime
        .start(
            claude.id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: cwd.clone(),
                env: BTreeMap::new(),
                env_remove: Vec::new(),
            },
        )
        .await
        .unwrap();
    let (status, _) = agent_request(
        f.app(),
        "PUT",
        &path,
        Some(&token),
        Some("web"),
        json!({"choice":false}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    f.state.runtime.stop(&claude.id.to_string()).await.unwrap();
}

#[tokio::test]
async fn a_pending_approval_marks_the_session_waiting_until_the_last_is_resolved() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let workspace = f
        .state
        .store
        .create_workspace("approvals", f.path.join("repo").to_str().unwrap())
        .unwrap();
    let session = f
        .state
        .store
        .create_session(workspace.id, ProviderKind::ClaudeCode, "asks")
        .unwrap();
    let runtime = conversations::ChatRuntime::for_test();
    let status = || {
        f.state
            .store
            .get_session(session.id)
            .unwrap()
            .unwrap()
            .status
    };
    let approval = |id: &str| json!({"type":"approval","id":id,"title":"Run","text":"Fixture","choices":["accept","decline"]});
    let resolved = |id: &str| json!({"type":"approval_resolved","id":id});
    for (event, expected) in [
        (approval("a"), SessionStatus::Waiting),
        (approval("b"), SessionStatus::Waiting),
        (resolved("a"), SessionStatus::Waiting),
        (resolved("b"), SessionStatus::Running),
        (resolved("b"), SessionStatus::Running),
    ] {
        conversations::handle_event_for_test(&f.state, session.id, &runtime, event.clone())
            .await
            .unwrap();
        assert_eq!(
            format!("{:?}", status()),
            format!("{expected:?}"),
            "after {event}"
        );
    }
}

#[tokio::test]
async fn a_granted_file_outside_the_roots_can_be_previewed_until_revoked() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let outside = f.path.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("notes.log"), "outside the roots").unwrap();
    let file = dunce::canonicalize(outside.join("notes.log")).unwrap();
    let preview = format!("/api/host/file?path={}", url_escape(file.to_str().unwrap()));
    let raw = format!("/api/host/raw?path={}", url_escape(file.to_str().unwrap()));
    let (status, _) = call(f.app(), "GET", &preview, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "outside the browsing roots");
    let (status, _) = call(f.app(), "GET", &raw, Value::Null).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the raw bytes have the same reach"
    );

    let (status, grants) = call(
        f.app(),
        "POST",
        "/api/host/grants",
        json!({"path": file, "directory": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grants[0]["directory"], false);
    let (status, body) = call(f.app(), "GET", &preview, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "outside the roots");
    let (status, _) = call(f.app(), "GET", &raw, Value::Null).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an image or video there can be shown too"
    );

    let (status, _) = call(f.app(), "DELETE", "/api/host/grants/all", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(f.app(), "GET", &preview, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "revoked");
    let (status, _) = call(f.app(), "GET", &raw, Value::Null).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "revoked for the raw bytes too"
    );
}

fn url_escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[tokio::test]
async fn a_file_pasted_into_a_terminal_lands_in_that_sessions_directory() {
    let f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let id = register(&f).await;
    let (status, session) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{id}/sessions"),
        json!({"title":"Shell","provider":"terminal"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session = session["id"].as_str().unwrap();
    let (status, saved) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{session}/uploads?name=image.png"),
        json!("pixels"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let absolute = saved["absolute"].as_str().unwrap();
    let repo = dunce::canonicalize(f.path.join("repo")).unwrap();
    assert!(
        std::path::Path::new(absolute).starts_with(&repo),
        "{absolute}"
    );
    assert!(
        saved["path"]
            .as_str()
            .unwrap()
            .starts_with(".agentdock-files/pastes/")
    );
    let (status, _) = call(
        f.app(),
        "POST",
        &format!("/api/sessions/{}/uploads?name=a.png", uuid::Uuid::new_v4()),
        json!("x"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unknown session has nowhere to put it"
    );
}

#[tokio::test]
async fn resource_history_answers_from_the_metrics_database() {
    use agentdock_persistence::metrics::{HostPoint, MetricsStore, SessionPoint};
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let (status, _) = call(f.app(), "GET", "/api/host/resources/history", Value::Null).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "no metrics database, no history"
    );

    let store = Arc::new(MetricsStore::open(":memory:").unwrap());
    let now = chrono::Utc::now().timestamp();
    let host = |ts| HostPoint {
        ts,
        cpu_avg: 12.5,
        cpu_max: 12.5,
        memory_used_avg: 100,
        memory_used_max: 100,
        memory_total: 1000,
        fan_rpm_avg: Some(1200),
        fan_rpm_max: Some(1200),
        temperature_avg: Some(48.5),
        temperature_max: Some(48.5),
        cpu_mhz_avg: Some(3204),
        cpu_mhz_max: Some(3504),
    };
    let session = |ts, id: &str| SessionPoint {
        ts,
        session_id: id.into(),
        cpu_avg: 3.0,
        cpu_max: 3.0,
        memory_avg: 10,
        memory_max: 10,
    };
    store
        .record(
            &[host(now - 30), host(now - 25)],
            &[session(now - 30, "a"), session(now - 30, "b")],
        )
        .unwrap();
    f.state.metrics = Some(store);

    let (status, body) = call(f.app(), "GET", "/api/host/resources/history", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["resolution"], 5);
    assert_eq!(body["host"].as_array().unwrap().len(), 2);
    assert_eq!(body["host"][0]["cpu_avg"], 12.5);
    assert_eq!(body["host"][0]["fan_rpm_max"], 1200);
    assert_eq!(body["host"][0]["temperature_max"], 48.5);
    assert_eq!(body["host"][0]["cpu_mhz_max"], 3504);
    assert_eq!(body["sessions"].as_array().unwrap().len(), 2);

    let (_, body) = call(
        f.app(),
        "GET",
        "/api/host/resources/history?session_id=b",
        Value::Null,
    )
    .await;
    assert_eq!(body["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(body["sessions"][0]["session_id"], "b");

    let path = format!("/api/host/resources/history?from={now}&to={}", now - 1);
    let (status, _) = call(f.app(), "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resource_history_can_be_turned_off_and_cleared() {
    use agentdock_persistence::metrics::{HostPoint, MetricsStore};
    let mut f = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let store = Arc::new(MetricsStore::open(":memory:").unwrap());
    let now = chrono::Utc::now().timestamp();
    store
        .record(
            &[HostPoint {
                ts: now - 30,
                cpu_avg: 1.0,
                cpu_max: 1.0,
                memory_used_avg: 1,
                memory_used_max: 1,
                memory_total: 2,
                fan_rpm_avg: None,
                fan_rpm_max: None,
                temperature_avg: None,
                temperature_max: None,
                cpu_mhz_avg: None,
                cpu_mhz_max: None,
            }],
            &[],
        )
        .unwrap();
    f.state.metrics = Some(store);
    let history = "/api/host/resources/history";
    let (_, body) = call(f.app(), "GET", history, Value::Null).await;
    assert_eq!(body["recording"], true, "on unless turned off");

    let (status, preferences) = call(
        f.app(),
        "PUT",
        "/api/preferences",
        json!({"resource_history": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preferences["resource_history"], false);
    let (_, body) = call(f.app(), "GET", history, Value::Null).await;
    assert_eq!(body["recording"], false);
    assert_eq!(
        body["host"].as_array().unwrap().len(),
        1,
        "turning recording off keeps what was recorded"
    );

    let (status, _) = call(f.app(), "DELETE", history, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(f.app(), "GET", history, Value::Null).await;
    assert!(body["host"].as_array().unwrap().is_empty());
}
