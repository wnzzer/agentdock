use super::*;

fn session_fixture(fixture: &Fixture, provider: ProviderKind) -> Session {
    let workspace = fixture
        .state
        .store
        .create_workspace(
            "Archive fixture",
            fixture.path.join("repo").to_str().unwrap(),
        )
        .unwrap();
    fixture
        .state
        .store
        .create_session(workspace.id, provider, "Retained session")
        .unwrap()
}

fn archive_metadata_removed(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("archived_at");
    value.as_object_mut().unwrap().remove("updated_at");
    value
}

#[tokio::test]
async fn archive_api_is_reversible_idempotent_and_keeps_list_and_canvas_bindings() {
    let fixture = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let session = session_fixture(&fixture, ProviderKind::Codex);
    let id = session.id;
    fixture
        .state
        .store
        .set_interaction_mode(id, InteractionMode::Structured)
        .unwrap();
    fixture
        .state
        .store
        .set_native_conversation_id(id, "retained-native-id")
        .unwrap();
    let event = fixture
        .state
        .store
        .append_conversation_event(
            id,
            json!({"type":"message","role":"assistant","text":"Retain this conversation"}),
        )
        .unwrap();
    let canvas = canvas_document(vec![
        json!({"type":"pane","id":"session-pane","kind":"agent_chat","metadata":{"session_id":id,"workspace_id":session.workspace_id}}),
    ]);
    assert_eq!(
        call(
            fixture.app(),
            "PUT",
            "/api/canvas/layout",
            json!({"layout":canvas,"expected_revision":0})
        )
        .await
        .0,
        StatusCode::OK
    );
    let saved = fixture.state.store.get_shared_canvas_layout().unwrap();
    let (_, original) = call(
        fixture.app(),
        "GET",
        &format!("/api/sessions/{id}"),
        Value::Null,
    )
    .await;
    let url = format!("/api/sessions/{id}/archive");
    let (status, archived) = call(fixture.app(), "PATCH", &url, json!({"archived":true})).await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert!(
        chrono::DateTime::parse_from_rfc3339(archived["archived_at"].as_str().unwrap()).is_ok()
    );
    assert_eq!(
        archive_metadata_removed(archived.clone()),
        archive_metadata_removed(original)
    );
    assert_eq!(
        call(fixture.app(), "PATCH", &url, json!({"archived":true})).await,
        (StatusCode::OK, archived.clone())
    );
    for url in [
        "/api/sessions".to_owned(),
        format!("/api/sessions?workspace_id={}", session.workspace_id),
    ] {
        let (status, records) = call(fixture.app(), "GET", &url, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(records, json!([archived]));
    }
    assert_eq!(
        fixture.state.store.get_shared_canvas_layout().unwrap(),
        saved
    );
    assert_eq!(
        fixture.state.store.conversation(id).unwrap().events,
        vec![event.clone()]
    );
    assert!(fixture.state.runtime.get(&id.to_string()).is_none());
    assert!(fixture.state.chats.get(id).is_none());
    let (status, restored) = call(fixture.app(), "PATCH", &url, json!({"archived":false})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(restored["archived_at"].is_null());
    assert_eq!(restored["id"], json!(id));
    assert_eq!(restored["provider_session_id"], "retained-native-id");
    assert_eq!(
        call(fixture.app(), "PATCH", &url, json!({"archived":false})).await,
        (StatusCode::OK, restored)
    );
    assert_eq!(
        fixture.state.store.conversation(id).unwrap().events,
        vec![event]
    );
    assert_eq!(
        fixture.state.store.get_shared_canvas_layout().unwrap(),
        saved
    );
}

#[tokio::test]
async fn archive_api_does_not_stop_or_replace_a_running_native_process() {
    let fixture = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let session = session_fixture(&fixture, ProviderKind::Terminal);
    let id = session.id;
    let runtime = fixture
        .state
        .runtime
        .start(
            id.to_string(),
            agentdock_runtime::SpawnSpec {
                program: waiting_process(&["-i"]).0,
                args: waiting_process(&["-i"]).1,
                cwd: fixture.path.join("repo"),
                env: Default::default(),
                env_remove: vec![],
            },
        )
        .await
        .unwrap();
    fixture
        .state
        .store
        .set_session_status(id, SessionStatus::Running)
        .unwrap();
    let mut events = runtime.subscribe();
    runtime
        .input(announce("ARCHIVE", "FIXTURE").into_bytes())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut output = Vec::new();
        loop {
            if let RuntimeEvent::Output { data, .. } = events.recv().await.unwrap() {
                output.extend(data);
                if String::from_utf8_lossy(&output).contains("ARCHIVE_FIXTURE_READY") {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    let mut results = Vec::new();
    for archived in [true, false] {
        let response = call(
            fixture.app(),
            "PATCH",
            &format!("/api/sessions/{id}/archive"),
            json!({"archived":archived}),
        )
        .await;
        let same_process = fixture
            .state
            .runtime
            .get(&id.to_string())
            .is_some_and(|current| Arc::ptr_eq(&runtime, &current));
        let running = runtime.running();
        let output: Vec<_> = runtime
            .snapshot()
            .into_iter()
            .filter_map(|event| match event {
                RuntimeEvent::Output { data, .. } => Some(data),
                _ => None,
            })
            .flatten()
            .collect();
        results.push((
            response,
            same_process,
            running,
            String::from_utf8_lossy(&output).contains("ARCHIVE_FIXTURE_READY"),
        ));
    }
    // Only test cleanup terminates the fixture process.
    runtime.stop().await.unwrap();
    for ((status, response), same_process, running, replay_preserved) in results {
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["status"], "running");
        assert!(same_process);
        assert!(running);
        assert!(replay_preserved);
    }
}

#[tokio::test]
async fn archive_api_validates_payload_and_session_identity_without_side_effects() {
    let fixture = Fixture::new("127.0.0.1:8787".parse().unwrap(), None);
    let session = session_fixture(&fixture, ProviderKind::ClaudeCode);
    let url = format!("/api/sessions/{}/archive", session.id);
    for input in [
        json!({}),
        json!({"archived":"true"}),
        json!({"archived":null}),
        json!({"archived":true,"status":"stopped"}),
    ] {
        assert_eq!(
            call(fixture.app(), "PATCH", &url, input).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for archived in [true, false] {
        let (status, body) = call(
            fixture.app(),
            "PATCH",
            &format!("/api/sessions/{}/archive", Uuid::new_v4()),
            json!({"archived":archived}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "Session not found");
    }
    assert_eq!(
        call(
            fixture.app(),
            "PATCH",
            "/api/sessions/not-a-uuid/archive",
            json!({"archived":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        serde_json::to_value(
            fixture
                .state
                .store
                .get_session(session.id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(session).unwrap()
    );
}

#[tokio::test]
async fn archive_api_uses_existing_authentication_and_csrf_guards() {
    let fixture = Fixture::new(
        "127.0.0.1:8787".parse().unwrap(),
        Some("fixture-token-long-enough"),
    );
    let session = session_fixture(&fixture, ProviderKind::Codex);
    let url = format!("/api/sessions/{}/archive", session.id);
    assert_eq!(
        call(fixture.app(), "PATCH", &url, json!({"archived":true}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for (client_header, origin, expected) in [
        (false, "http://127.0.0.1:5173", StatusCode::FORBIDDEN),
        (true, "https://attacker.invalid", StatusCode::FORBIDDEN),
        (true, "http://127.0.0.1:5173", StatusCode::OK),
    ] {
        let mut request = Request::builder()
            .method("PATCH")
            .uri(&url)
            .header("host", "127.0.0.1:8787")
            .header("authorization", "Bearer fixture-token-long-enough")
            .header("content-type", "application/json")
            .header("origin", origin);
        if client_header {
            request = request.header("x-agentdock-client", "web");
        }
        let request = request
            .body(Body::from(json!({"archived":true}).to_string()))
            .unwrap();
        assert_eq!(
            fixture.app().oneshot(request).await.unwrap().status(),
            expected
        );
        assert_eq!(
            fixture
                .state
                .store
                .get_session(session.id)
                .unwrap()
                .unwrap()
                .archived_at
                .is_some(),
            expected == StatusCode::OK
        );
    }
}
