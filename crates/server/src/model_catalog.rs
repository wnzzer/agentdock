//! Read-only model discovery. Explicit UI action only; never runs inference.
use crate::{ApiError, providers};
use agentdock_domain::{EndpointProfile, ProviderKind};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};
use url::Url;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    /// Reasoning levels this model advertised. Empty means the catalog said
    /// nothing, so no effort choice is offered rather than a guessed one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct ModelCatalog {
    pub models: Vec<ModelEntry>,
    pub source_url: String,
    pub has_more: bool,
}

pub fn list_url(profile: &EndpointProfile) -> Result<Url, ApiError> {
    let default = match profile.provider {
        ProviderKind::ClaudeCode => "https://api.anthropic.com",
        ProviderKind::Codex => "https://api.openai.com/v1",
        ProviderKind::Terminal => return Err(ApiError::bad("Terminal has no model list")),
    };
    let mut url = Url::parse(profile.endpoint_url.as_deref().unwrap_or(default))
        .map_err(|_| ApiError::bad("Invalid endpoint URL"))?;
    if matches!(
        url.host_str(),
        Some("169.254.169.254" | "metadata.google.internal")
    ) {
        return Err(ApiError::bad(
            "Cloud metadata addresses cannot be used as model endpoints",
        ));
    }
    let path = url.path().trim_end_matches('/');
    let suffix = if path.is_empty()
        || (profile.provider == ProviderKind::ClaudeCode && !path.ends_with("/v1"))
    {
        "/v1/models"
    } else {
        "/models"
    };
    url.set_path(&format!("{path}{suffix}"));
    Ok(url)
}
fn parse_models(value: Value, source_url: String) -> Result<ModelCatalog, ApiError> {
    let rows = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| {
            ApiError::bad(
                "The endpoint did not return a supported model list; enter a model ID manually",
            )
        })?;
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for row in rows.iter().take(2000) {
        let Some(id) = row
            .get("id")
            .or_else(|| row.get("model"))
            .or_else(|| row.get("name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 200 && !s.chars().any(char::is_control))
        else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let name = row
            .get("display_name")
            .or_else(|| row.get("displayName"))
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 300 && !s.chars().any(char::is_control))
            .unwrap_or(id);
        // Codex reports supported levels per model; anything outside the set
        // both clients accept is dropped rather than offered.
        let efforts = row
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(|level| {
                        level
                            .get("reasoningEffort")
                            .or(Some(level))
                            .and_then(Value::as_str)
                    })
                    .filter(|level| providers::EFFORT_LEVELS.contains(level))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        models.push(ModelEntry {
            id: id.into(),
            name: name.into(),
            efforts,
        });
    }
    if models.is_empty() {
        return Err(ApiError::bad(
            "No model IDs were returned; enter a model ID manually",
        ));
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(ModelCatalog {
        models,
        source_url,
        has_more: rows.len() > 2000
            || value
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false),
    })
}
pub async fn discover(
    state_dir: &std::path::Path,
    profile: &EndpointProfile,
) -> Result<ModelCatalog, ApiError> {
    if profile.native_config.is_some() {
        return Err(ApiError::bad(
            "Models for an existing native configuration stay managed by the native client",
        ));
    }
    providers::validate_profile(profile)?;
    if profile.provider == ProviderKind::Codex
        && profile.endpoint_url.is_none()
        && profile.secret_ref.is_none()
    {
        return native_codex_models(state_dir, profile).await;
    }
    let secret =
        if let Some(reference) = profile.secret_ref.as_deref() {
            let key = reference
                .strip_prefix("env:")
                .ok_or_else(|| ApiError::bad("Invalid secret reference"))?;
            Some(std::env::var(key).map_err(|_| {
                ApiError::bad("The profile secret reference is not set on the server")
            })?)
        } else {
            None
        };
    fetch(profile, secret.as_deref()).await
}

async fn native_codex_models(
    state_dir: &std::path::Path,
    profile: &EndpointProfile,
) -> Result<ModelCatalog, ApiError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory = providers::private_dir(
        &std::env::temp_dir().join(format!("agentdock-models-{}", uuid::Uuid::new_v4())),
    )?;
    let mut command =
        tokio::process::Command::new(crate::clients::program(state_dir, &ProviderKind::Codex));
    command
        .args(["app-server", "-c", "cli_auth_credentials_store=\"file\""])
        .current_dir(&directory)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (key, _) in std::env::vars().filter(|(k, _)| {
        k.starts_with("OPENAI_")
            || k.starts_with("CODEX_")
            || k.starts_with("ANTHROPIC_")
            || k.starts_with("AGENTDOCK_SECRET_")
            || k == "AGENTDOCK_TOKEN"
    }) {
        command.env_remove(key);
    }
    command.env("CODEX_HOME", &directory);
    if let Some(proxy) = &profile.proxy_url {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, proxy);
        }
        command.env("NO_PROXY", "").env("no_proxy", "");
    }
    let result=async {
        let mut child=command.spawn().map_err(|_|ApiError::bad("Codex is not installed on the server; enter a model ID manually"))?;
        let mut input=child.stdin.take().ok_or_else(||ApiError::bad("Unable to open Codex input"))?;
        let mut output=child.stdout.take().ok_or_else(||ApiError::bad("Unable to open Codex output"))?;
        let query=async {
            input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"agentdock_models\",\"version\":\"0.1.0\"}}}\n").await.map_err(ApiError::internal)?;
            let mut buffered=Vec::new();let mut chunk=[0u8;8192];let mut initialized=false;
            loop {
                let n=output.read(&mut chunk).await.map_err(ApiError::internal)?;
                if n==0{return Err(ApiError::bad("Codex model discovery ended without a model list"));}
                buffered.extend_from_slice(&chunk[..n]);if buffered.len()>MAX_BYTES{return Err(ApiError::bad("Native model list exceeds limit"));}
                while let Some(index)=buffered.iter().position(|b|*b==b'\n') {
                    let line:Vec<_>=buffered.drain(..=index).collect();let Ok(packet)=serde_json::from_slice::<Value>(&line)else{continue;};
                    if packet.get("id")==Some(&Value::from(1)) && !initialized {
                        if packet.get("error").is_some(){return Err(ApiError::bad("Codex app-server initialization failed"));}
                        input.write_all(b"{\"method\":\"initialized\",\"params\":{}}\n{\"id\":2,\"method\":\"model/list\",\"params\":{\"limit\":100,\"includeHidden\":false}}\n").await.map_err(ApiError::internal)?;initialized=true;
                    }
                    if packet.get("id")==Some(&Value::from(2)) {
                        let result=packet.get("result").ok_or_else(||ApiError::bad("This Codex version cannot list models"))?;
                        let mut catalog=parse_models(result.clone(),"codex://model/list".into())?;
                        catalog.has_more=result.get("nextCursor").is_some_and(|c|!c.is_null());
                        return Ok(catalog);
                    }
                }
            }
        };
        let result=match tokio::time::timeout(Duration::from_secs(8),query).await { Ok(result)=>result, Err(_)=>Err(ApiError::bad("Codex model discovery timed out")) };
        let _=child.kill().await;let _=child.wait().await;result
    }.await;
    // This directory was created solely for read-only discovery; never remove real session state.
    let _ = tokio::fs::remove_dir_all(&directory).await;
    result
}
async fn fetch(profile: &EndpointProfile, secret: Option<&str>) -> Result<ModelCatalog, ApiError> {
    let url = list_url(profile)?;
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy) = profile.proxy_url.as_deref() {
        builder = builder
            .no_proxy()
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| ApiError::bad("Invalid HTTP(S) proxy"))?);
    }
    let client = builder
        .build()
        .map_err(|_| ApiError::bad("Unable to configure the model-list connection"))?;
    let mut request = client.get(url.clone()).header("Accept", "application/json");
    if profile.provider == ProviderKind::ClaudeCode {
        request = request.header("anthropic-version", "2023-06-01");
        if let Some(key) = secret {
            request = request.header("x-api-key", key);
        }
    } else if let Some(key) = secret {
        request = request.bearer_auth(key);
    }
    // Never return upstream error bodies: they may include echoed credentials.
    let mut response = request.send().await.map_err(|_| {
        ApiError::bad("Cannot connect to the endpoint. Check the URL, proxy and server network.")
    })?;
    let status = response.status();
    if status.is_redirection() {
        return Err(ApiError::bad(
            "Model endpoint redirects are blocked to protect credentials. Enter the final base URL.",
        ));
    }
    if !status.is_success() {
        return Err(ApiError::bad(format!(
            "Model list request failed (HTTP {}). Check API-key access; subscription login cannot be reused by this API request.",
            status.as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(ApiError::bad("Model list response exceeds 2 MiB"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::bad("Model list download failed"))?
    {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err(ApiError::bad("Model list response exceeds 2 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| {
        ApiError::bad("Model endpoint returned non-JSON content; enter a model ID manually")
    })?;
    parse_models(value, url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    fn profile(url: String) -> EndpointProfile {
        EndpointProfile {
            id: uuid::Uuid::new_v4(),
            name: "fixture".into(),
            provider: ProviderKind::Codex,
            endpoint_url: Some(url),
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: None,
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        }
    }
    #[tokio::test]
    async fn returns_real_ids_and_uses_auth_without_inference() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 8192];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            assert!(request.starts_with("get /v1/models "));
            assert!(request.contains("authorization: bearer fixture-only"));
            let body = r#"{"data":[{"id":"actual-model-b"},{"id":"actual-model-a","display_name":"Actual A"},{"id":"actual-model-a"}]}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        });
        let result = fetch(
            &profile(format!("http://{address}/v1")),
            Some("fixture-only"),
        )
        .await
        .unwrap();
        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].id, "actual-model-a");
        server.await.unwrap();
    }
    #[test]
    fn url_and_alias_rules() {
        let mut p = profile("https://example.test".into());
        assert_eq!(list_url(&p).unwrap().path(), "/v1/models");
        p.endpoint_url = Some("https://example.test/prefix/v1".into());
        assert_eq!(list_url(&p).unwrap().path(), "/prefix/v1/models");
        p.provider = ProviderKind::ClaudeCode;
        p.endpoint_url = Some("https://example.test/gateway".into());
        assert_eq!(list_url(&p).unwrap().path(), "/gateway/v1/models");
        p.model = Some("fast".into());
        p.model_aliases
            .insert("fast".into(), "real-model-id".into());
        assert_eq!(
            providers::resolve_model(&p).as_deref(),
            Some("real-model-id")
        );
        p.proxy_url = Some("socks5://127.0.0.1:1080".into());
        assert!(providers::validate_profile(&p).is_err());
    }

    #[test]
    fn keeps_only_advertised_reasoning_levels() {
        let catalog = parse_models(
            serde_json::json!({"data":[
                {"id":"reasoning-model","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"},{"reasoningEffort":"unsupported"},"medium"]}
            ]}),
            "fixture://models".into(),
        ).unwrap();
        assert_eq!(catalog.models[0].efforts, ["low", "high", "medium"]);
    }
    #[tokio::test]
    async fn rejects_redirect_and_does_not_forward_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut b = [0; 4096];
            let read = s.read(&mut b).await.unwrap();
            assert!(read > 0);
            s.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/secret\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let error = fetch(&profile(format!("http://{addr}/v1")), Some("fixture-only"))
            .await
            .unwrap_err();
        assert!(error.message.contains("redirect"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn selected_http_proxy_routes_the_model_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 8192];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            assert!(request.starts_with("get http://models.invalid/v1/models "));
            let body = r#"{"data":[{"id":"proxy-model"}]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let mut p = profile("http://models.invalid/v1".into());
        p.proxy_url = Some(format!("http://{address}"));
        assert_eq!(fetch(&p, None).await.unwrap().models[0].id, "proxy-model");
        server.await.unwrap();
    }
}
