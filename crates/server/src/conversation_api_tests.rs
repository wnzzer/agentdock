use super::*;

fn chat_fixture() -> Fixture {
    let mut fixture = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    fixture.state.chat_bridge = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/native-bridge/fixtures/chat-supervisor.mjs");
    fixture
}
async fn new_chat(fixture: &Fixture) -> Uuid {
    let workspace = fixture
        .state
        .store
        .create_workspace("Chat fixture", fixture.path.join("repo").to_str().unwrap())
        .unwrap();
    let (status,session)=call(fixture.app(),"POST",&format!("/api/workspaces/{}/sessions",workspace.id),json!({"provider":"codex","title":"Fixture chat","interaction_mode":"structured","environment":{"OLD_ACCOUNT_VALUE":{"kind":"literal","value":"not-carried"}}})).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    assert_eq!(session["interaction_mode"], "structured");
    Uuid::parse_str(session["id"].as_str().unwrap()).unwrap()
}
async fn wait_event(fixture: &Fixture, id: Uuid, kind: &str, status: Option<&str>) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let events = fixture.state.store.conversation(id).unwrap().events;
            if events
                .iter()
                .any(|e| e["type"] == kind && status.is_none_or(|s| e["status"] == s))
            {
                return events;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture event deadline")
}

#[tokio::test]
async fn structured_conversation_open_is_prompt_free_and_messages_are_durable_and_idempotent() {
    let fixture = chat_fixture();
    let id = new_chat(&fixture).await;
    let (status, snapshot) = call(
        fixture.app(),
        "GET",
        &format!("/api/sessions/{id}/conversation"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["mode"], "structured");
    assert_eq!(snapshot["running"], false);
    assert_eq!(snapshot["events"], json!([]));
    let (status, session) = call(
        fixture.app(),
        "POST",
        &format!("/api/sessions/{id}/start"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["status"], "running");
    let initial = fixture.state.store.conversation(id).unwrap().events;
    assert!(initial.iter().all(|e| e["type"] == "ready"));
    let message_id = Uuid::new_v4().to_string();
    let request = json!({"id":message_id,"content":"HELLO"});
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/conversation/message"),
            request.clone()
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let events = wait_event(&fixture, id, "turn", Some("completed")).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "message" && e["role"] == "user")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "message" && e["role"] == "assistant")
            .map(|e| e["text"].as_str().unwrap())
            .collect::<String>(),
        "fixture reply"
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/conversation/message"),
            request
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        fixture.state.store.conversation(id).unwrap().events.len(),
        events.len()
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/conversation/message"),
            json!({"id":message_id,"content":"different"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/stop"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(!fixture.state.chats.get(id).unwrap().running());
    assert_eq!(
        fixture
            .state
            .store
            .get_session(id)
            .unwrap()
            .unwrap()
            .provider_session_id
            .as_deref(),
        Some("fixture-native-thread")
    );
    assert!(
        !fixture
            .state
            .store
            .conversation(id)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn native_approvals_require_explicit_supported_decisions_and_block_endpoint_switches() {
    let fixture = chat_fixture();
    let id = new_chat(&fixture).await;
    let base = format!("/api/sessions/{id}");
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/message"),
            json!({"id":Uuid::new_v4(),"content":"APPROVAL"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait_event(&fixture, id, "approval", None).await;
    assert_eq!(
        call(
            fixture.app(),
            "PATCH",
            &(base.clone() + "/configuration"),
            json!({"endpoint_profile_id":null,"confirmed":true})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/approval"),
            json!({"request_id":"fixture-approval","decision":"acceptForSession"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/approval"),
            json!({"request_id":"other-session-request","decision":"accept"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/approval"),
            json!({"request_id":"fixture-approval","decision":"decline"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let events = wait_event(&fixture, id, "turn", Some("completed")).await;
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "tool" && e["status"] == "failed" && e["text"] == "decline")
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/approval"),
            json!({"request_id":"fixture-approval","decision":"accept"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    fixture.state.chats.stop(id).await.unwrap();
}

#[tokio::test]
async fn interrupt_cancels_a_turn_not_the_session_and_allows_a_confirmed_fresh_endpoint_context() {
    let fixture = chat_fixture();
    let id = new_chat(&fixture).await;
    let base = format!("/api/sessions/{id}");
    let (_,profile)=call(fixture.app(),"POST","/api/endpoint-profiles",json!({"name":"Next account","provider":"codex","endpoint_url":"https://fixture.invalid/v1","environment":{"NEW_ACCOUNT_VALUE":{"kind":"literal","value":"new"}}})).await;
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/message"),
            json!({"id":Uuid::new_v4(),"content":"WAIT"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait_event(&fixture, id, "turn", Some("running")).await;
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/message"),
            json!({"id":Uuid::new_v4(),"content":"must-not-queue"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/interrupt"),
            json!({})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait_event(&fixture, id, "turn", Some("interrupted")).await;
    assert!(fixture.state.chats.get(id).unwrap().running());
    assert_eq!(
        call(
            fixture.app(),
            "PATCH",
            &(base.clone() + "/configuration"),
            json!({"endpoint_profile_id":profile["id"],"confirmed":false})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, changed) = call(
        fixture.app(),
        "PATCH",
        &(base.clone() + "/configuration"),
        json!({"endpoint_profile_id":profile["id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["status"], "stopped");
    assert_eq!(changed["configuration_revision"], 1);
    assert_eq!(changed["provider_session_id"], Value::Null);
    assert_eq!(
        changed["environment"],
        json!({"NEW_ACCOUNT_VALUE":{"kind":"literal","value":"new"}})
    );
    assert!(!fixture.state.chats.get(id).unwrap().running());
    assert!(
        fixture
            .state
            .store
            .conversation(id)
            .unwrap()
            .events
            .iter()
            .any(|e| e["type"] == "configuration")
    );
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &(base.clone() + "/conversation/message"),
            json!({"id":Uuid::new_v4(),"content":"HELLO"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert!(
        fixture
            .state
            .state_dir
            .join("sessions")
            .join(id.to_string())
            .join("configurations/1/config.toml")
            .exists()
    );
    fixture.state.chats.stop(id).await.unwrap();
}

#[tokio::test]
async fn chat_conversion_and_configuration_do_not_take_over_a_live_pty_or_change_cli() {
    let fixture = chat_fixture();
    let workspace = fixture
        .state
        .store
        .create_workspace("Fixture", fixture.path.join("repo").to_str().unwrap())
        .unwrap();
    let record = fixture
        .state
        .store
        .create_session(workspace.id, ProviderKind::Codex, "Existing PTY")
        .unwrap();
    let id = record.id;
    let spec = agentdock_runtime::SpawnSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "read line".into()],
        cwd: fixture.path.join("repo"),
        env: Default::default(),
        env_remove: vec![],
    };
    fixture
        .state
        .runtime
        .start(id.to_string(), spec)
        .await
        .unwrap(); // DB deliberately still stopped: live-runtime check must win.
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/conversation/open"),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            fixture.app(),
            "PATCH",
            &format!("/api/sessions/{id}/configuration"),
            json!({"endpoint_profile_id":null,"confirmed":true})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        fixture
            .state
            .runtime
            .get(&id.to_string())
            .unwrap()
            .running()
    );
    fixture.state.runtime.shutdown().await;
    let (_, foreign) = call(
        fixture.app(),
        "POST",
        "/api/endpoint-profiles",
        json!({"name":"Other CLI","provider":"claude_code"}),
    )
    .await;
    assert_eq!(
        call(
            fixture.app(),
            "PATCH",
            &format!("/api/sessions/{id}/configuration"),
            json!({"endpoint_profile_id":foreign["id"],"confirmed":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .state
            .store
            .get_session(id)
            .unwrap()
            .unwrap()
            .interaction_mode,
        InteractionMode::Pty
    );
}

#[tokio::test]
async fn chat_and_configuration_routes_require_existing_authentication() {
    let fixture = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("fixture-auth-token-long-enough"),
    );
    let id = Uuid::new_v4();
    for (method, suffix, body) in [
        ("GET", "conversation", Value::Null),
        ("POST", "conversation/open", json!({})),
        (
            "POST",
            "conversation/message",
            json!({"id":Uuid::new_v4(),"content":"not sent"}),
        ),
        (
            "PATCH",
            "configuration",
            json!({"confirmed":true,"endpoint_profile_id":null}),
        ),
    ] {
        assert_eq!(
            call(
                fixture.app(),
                method,
                &format!("/api/sessions/{id}/{suffix}"),
                body
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn actual_node_bridge_contract_runs_both_synthetic_native_protocols_end_to_end() {
    for provider in ["codex", "claude_code"] {
        let mut fixture = chat_fixture();
        let cli = fixture.path.join("repo/fake-chat-cli.mjs");
        fs::write(
            &cli,
            include_str!("../../../packages/native-bridge/fixtures/chat-cli.mjs"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let wrapper = fixture.path.join("bridge-fixture.mjs");
        let fixture_url = url::Url::from_file_path(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/native-bridge/fixtures/chat-supervised-bridge.mjs"),
        )
        .unwrap();
        let log = fixture.path.join("native.jsonl");
        // Only this test child gets fixture variables. Never change the test
        // runner's process environment or resolve an actual account secret.
        fs::write(&wrapper,format!("process.env.AGENTDOCK_CHAT_TEST_INHERITED='kept';\nprocess.env.AGENTDOCK_CHAT_TEST_LOG={};\nprocess.env.AGENTDOCK_SECRET_TEST_CREDENTIAL='synthetic-fixture-secret';\nawait import({});\n",json!(log),json!(fixture_url.as_str()))).unwrap();
        fixture.state.chat_bridge = wrapper;
        let workspace = fixture
            .state
            .store
            .create_workspace(
                "Full stack fixture",
                fixture.path.join("repo").to_str().unwrap(),
            )
            .unwrap();
        let (status, session) = call(
            fixture.app(),
            "POST",
            &format!("/api/workspaces/{}/sessions", workspace.id),
            json!({"provider":provider,"title":"Bridge fixture","interaction_mode":"structured"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{session}");
        let id = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
        let (status, body) = call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/start"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{provider}: {body}");
        let before = fs::read_to_string(&log).unwrap();
        assert!(!before.contains("turn/start"));
        assert!(!before.contains("thread/start"));
        assert!(!before.contains("\"type\":\"user\""));
        let message_id = Uuid::new_v4();
        assert_eq!(
            call(
                fixture.app(),
                "POST",
                &format!("/api/sessions/{id}/conversation/message"),
                json!({"id":message_id,"content":"simple"})
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
        let events = wait_event(&fixture, id, "turn", Some("completed")).await;
        assert!(
            events.iter().any(|event| event["type"] == "message"
                && event["role"] == "assistant"
                && event["delta"] == false
                && event["text"] == "Hello 🦊"),
            "{provider}: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| event["role"] == "user" && event["id"] == message_id.to_string())
        );
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("synthetic-fixture-secret")
        );
        assert!(
            fixture
                .state
                .store
                .get_session(id)
                .unwrap()
                .unwrap()
                .provider_session_id
                .is_some()
        );
        fixture.state.chats.stop(id).await.unwrap();
    }
}

#[tokio::test]
async fn structured_websocket_only_attaches_and_disconnection_keeps_native_process_alive() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut fixture = chat_fixture();
    fixture.state.security = security::Security::for_test(address, None);
    let workspace = fixture
        .state
        .store
        .create_workspace(
            "Socket fixture",
            fixture.path.join("repo").to_str().unwrap(),
        )
        .unwrap();
    let session = fixture
        .state
        .store
        .create_session(workspace.id, ProviderKind::Codex, "Socket fixture")
        .unwrap();
    let id = session.id;
    fixture
        .state
        .store
        .set_interaction_mode(id, InteractionMode::Structured)
        .unwrap();
    let app = fixture.app();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("ws://{address}/api/sessions/{id}/chat/ws");
    let (mut ws, _) = connect_async(&url).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(snapshot["running"], false);
    assert!(fixture.state.chats.get(id).is_none());
    ws.close(None).await.unwrap();
    {
        let _guard = fixture.state.operations.lock().await;
        conversations::start_locked(
            &fixture.state,
            &fixture.state.store.get_session(id).unwrap().unwrap(),
        )
        .await
        .unwrap();
    }
    let (mut attached, _) = connect_async(&url).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(2), attached.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(snapshot["running"], true);
    assert_eq!(snapshot["events"][0]["type"], "ready");
    attached.close(None).await.unwrap();
    assert!(fixture.state.chats.get(id).unwrap().running());
    let (mut again, _) = connect_async(&url).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), again.next())
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    again.close(None).await.unwrap();
    fixture.state.chats.stop(id).await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn managed_account_profiles_cannot_be_orphaned_by_generic_profile_deletion() {
    let fixture = chat_fixture();
    fs::create_dir_all(&fixture.state.state_dir).unwrap();
    let (status, account) = call(
        fixture.app(),
        "POST",
        "/api/accounts",
        json!({"name":"Fixture managed account","provider":"codex"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{account}");
    let profile = account["profile_id"].as_str().unwrap();
    assert_eq!(
        call(
            fixture.app(),
            "DELETE",
            &format!("/api/endpoint-profiles/{profile}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        fixture
            .state
            .store
            .get_endpoint_profile(Uuid::parse_str(profile).unwrap())
            .unwrap()
            .is_some()
    );
    assert_eq!(account["status"], "unknown"); // Creation is not a login or account probe.
}

#[tokio::test]
async fn typed_command_receipts_are_readable_and_stale_configuration_cannot_dispatch_a_message() {
    let fixture = chat_fixture();
    let id = new_chat(&fixture).await;
    let path = format!("/api/sessions/{id}/conversation/message");
    fixture
        .state
        .store
        .switch_session_configuration(id, None, None)
        .unwrap();
    let message_id = Uuid::new_v4();
    let (status, _) = call(
        fixture.app(),
        "POST",
        &path,
        json!({"id":message_id,"content":"not another account","configuration_revision":0}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(fixture.state.chats.get(id).is_none());
    let input = json!({"id":message_id,"content":"intended account","configuration_revision":1});
    let (status, receipt) = call(fixture.app(), "POST", &path, input.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["id"], message_id.to_string());
    assert_eq!(receipt["duplicate"], false);
    let (_, repeat) = call(fixture.app(), "POST", &path, input).await;
    assert_eq!(repeat["duplicate"], true);
    let (_, interrupt) = call(
        fixture.app(),
        "POST",
        &format!("/api/sessions/{id}/conversation/interrupt"),
        json!({}),
    )
    .await;
    assert_eq!(interrupt["accepted"], true);
    fixture.state.chats.stop(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn bridge_exit_closes_its_owned_group_and_records_only_one_exit() {
    let mut fixture = chat_fixture();
    let bridge = fixture.path.join("owned-group-fixture.mjs");
    fs::write(&bridge,r#"import {spawn} from 'node:child_process';
import {writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {createInterface} from 'node:readline';
process.on('SIGTERM',()=>process.exit(0));
for await (const line of createInterface({input:process.stdin})) {
  const job=JSON.parse(line);
  if(job.type==='init') {
    const child=spawn('/bin/sh',['-c','trap "" TERM; printf ready; while :; do sleep 10; done'],{stdio:['ignore','pipe','ignore']});
    writeFileSync(join(job.cwd,'owned-child.pid'),String(child.pid));
    child.stdout.once('data',()=>process.stdout.write('{"type":"ready"}\n'));
  }
  if(job.type==='message') process.stdout.write('{"type":"exit"}\n');
  if(job.type==='shutdown') process.exit(0);
}
"#).unwrap();
    fixture.state.chat_bridge = bridge;
    let id = new_chat(&fixture).await;
    assert_eq!(
        call(
            fixture.app(),
            "POST",
            &format!("/api/sessions/{id}/conversation/message"),
            json!({"id":Uuid::new_v4(),"content":"synthetic exit"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let events = wait_event(&fixture, id, "exit", None).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "exit")
            .count(),
        1
    );
    assert!(!fixture.state.chats.get(id).unwrap().running());
    let pid = fs::read_to_string(fixture.path.join("repo/owned-child.pid")).unwrap();
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&output.stdout);
    assert!(
        status.trim().is_empty() || status.trim().starts_with('Z'),
        "owned descendant still executing: {status}"
    );
}
