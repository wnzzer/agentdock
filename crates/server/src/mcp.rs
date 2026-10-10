//! `agentdock mcp`: the AgentDock tools as a stdio MCP server.
//!
//! This process only relays. It speaks MCP (JSON-RPC 2.0, one message per
//! line) to whichever client launched it and forwards the tool list and each
//! tool call to the running AgentDock server, which decides everything
//! (agent.rs). It never opens the database or binds a port.
//!
//! Inside a session AgentDock launched, `AGENTDOCK_URL` and
//! `AGENTDOCK_AGENT_TOKEN` say where the server is and which session is
//! calling. Anywhere else it finds the local server the way `agentdock status`
//! does, with the deployment's own token when there is one.
use serde_json::{Value, json};
use std::{env, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};

/// Versions this server can answer as. The tools use nothing newer than the
/// first, so a client asking for any of these gets its own version back.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

struct Upstream {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
}

impl Upstream {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.url))
            .header("x-agentdock-client", "agent");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        request
    }
    async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = request.send().await.map_err(|_| {
            format!(
                "AgentDock is not answering at {}. Start it with `agentdock`.",
                self.url
            )
        })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            Err(body["error"]
                .as_str()
                .map_or_else(|| format!("AgentDock answered {status}"), str::to_owned))
        }
    }
    async fn tools(&self) -> Result<Value, String> {
        self.json(
            self.request(reqwest::Method::GET, "/api/agent/tools")
                .timeout(Duration::from_secs(20)),
        )
        .await
    }
    /// No timeout: a call that needs the person's confirmation waits for it,
    /// and the server bounds that wait.
    async fn call(&self, params: &Value) -> Value {
        let body = json!({ "name": params["name"], "arguments": params.get("arguments").cloned().unwrap_or(json!({})) });
        match self
            .json(
                self.request(reqwest::Method::POST, "/api/agent/call")
                    .json(&body),
            )
            .await
        {
            Ok(result) => result,
            Err(message) => {
                json!({ "content": [{ "type": "text", "text": message }], "isError": true })
            }
        }
    }
}

/// Where the local server answers: the address a session AgentDock launched is
/// given, or else this machine's port, as `agentdock status` finds it.
pub(crate) fn local_url() -> String {
    let url = env::var("AGENTDOCK_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| {
            let port = env::var("AGENTDOCK_ADDR")
                .ok()
                .and_then(|address| address.rsplit(':').next().map(str::to_owned))
                .unwrap_or_else(|| {
                    crate::DEFAULT_ADDRESS
                        .rsplit(':')
                        .next()
                        .unwrap_or("")
                        .to_owned()
                });
            format!("http://127.0.0.1:{port}")
        });
    url.trim_end_matches('/').to_owned()
}

fn locate() -> Upstream {
    let injected = env::var("AGENTDOCK_URL").ok().filter(|url| !url.is_empty());
    let url = local_url();
    let token = env::var("AGENTDOCK_AGENT_TOKEN")
        .ok()
        .filter(|token| injected.is_some() && !token.is_empty())
        .or_else(crate::security::existing_token);
    Upstream {
        client: reqwest::Client::new(),
        url,
        token,
    }
}

fn reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}
fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn answer(upstream: &Upstream, message: Value) -> Option<Value> {
    // A notification has no id and gets no answer, whatever it says.
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match message["method"].as_str().unwrap_or("") {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|version| **version == asked)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            // Instructions come from the server, so they describe the tools it
            // actually has. Unreachable now is not a reason to refuse to start.
            let instructions = upstream
                .tools()
                .await
                .ok()
                .and_then(|tools| tools["instructions"].as_str().map(str::to_owned))
                .unwrap_or_else(|| crate::agent::INSTRUCTIONS.to_owned());
            reply(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "agentdock", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": instructions,
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => match upstream.tools().await {
            Ok(tools) => reply(id, json!({ "tools": tools["tools"] })),
            Err(message) => failure(id, -32603, &message),
        },
        "tools/call" => reply(id, upstream.call(&params).await),
        _ => failure(id, -32601, "Method not found"),
    })
}

pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let upstream = Arc::new(locate());
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let message = match serde_json::from_str::<Value>(&line) {
            Ok(message) => message,
            Err(_) => {
                let text = failure(Value::Null, -32700, "Parse error").to_string();
                let mut out = stdout.lock().await;
                out.write_all(format!("{text}\n").as_bytes()).await?;
                out.flush().await?;
                continue;
            }
        };
        // Each request on its own task: a call waiting for a confirmation must
        // not hold up a ping or the next call.
        let (upstream, stdout) = (upstream.clone(), stdout.clone());
        tasks.spawn(async move {
            if let Some(response) = answer(&upstream, message).await {
                let mut out = stdout.lock().await;
                let _ = out.write_all(format!("{response}\n").as_bytes()).await;
                let _ = out.flush().await;
            }
        });
        while tasks.try_join_next().is_some() {}
    }
    // Stdin closed: let answers already under way finish before exiting.
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> Upstream {
        Upstream {
            client: reqwest::Client::new(),
            url: "http://127.0.0.1:9".into(),
            token: None,
        }
    }

    #[tokio::test]
    async fn initialize_negotiates_and_carries_instructions_even_offline() {
        let upstream = offline();
        let answer = answer(&upstream, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).await.unwrap();
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answer["result"]["serverInfo"]["name"], "agentdock");
        assert!(
            answer["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("agentdock_status")
        );
        let unknown = super::answer(&upstream, json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).await.unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[tokio::test]
    async fn notifications_get_no_answer_and_unknown_methods_an_error() {
        let upstream = offline();
        assert!(
            answer(
                &upstream,
                json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .await
            .is_none()
        );
        let unknown = answer(
            &upstream,
            json!({"jsonrpc":"2.0","id":"x","method":"resources/list"}),
        )
        .await
        .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        assert_eq!(
            answer(&upstream, json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
                .await
                .unwrap()["result"],
            json!({})
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_tool_error_the_model_can_read() {
        let upstream = offline();
        let call = answer(&upstream, json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"agentdock_status"}})).await.unwrap();
        assert_eq!(call["result"]["isError"], true);
        assert!(
            call["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not answering")
        );
        let list = answer(
            &upstream,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}),
        )
        .await
        .unwrap();
        assert_eq!(list["error"]["code"], -32603);
    }
}
