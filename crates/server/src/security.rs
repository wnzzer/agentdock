use crate::AppState;
use axum::{
    Json,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{collections::HashSet, env, net::SocketAddr};
use url::Url;

#[derive(Clone)]
pub struct Security {
    token: Option<String>,
    cookie: String,
    origins: HashSet<String>,
    hosts: HashSet<String>,
}
impl Security {
    pub fn new(address: SocketAddr) -> Result<Self, Box<dyn std::error::Error>> {
        let token = env::var("AGENTDOCK_TOKEN").ok().filter(|v| !v.is_empty());
        if !address.ip().is_loopback() && token.as_ref().is_none_or(|t| t.len() < 24) {
            return Err("Non-loopback binding requires AGENTDOCK_TOKEN (at least 24 characters) and an HTTPS reverse proxy".into());
        }
        let mut origins: HashSet<String> = ["http://127.0.0.1:5173", "http://localhost:5173"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        origins.insert(format!("http://127.0.0.1:{}", address.port()));
        origins.insert(format!("http://localhost:{}", address.port()));
        for value in env::var("AGENTDOCK_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
        {
            let parsed = Url::parse(value)?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err("Invalid allowed origin".into());
            }
            origins.insert(parsed.origin().ascii_serialization());
        }
        let hosts = origins
            .iter()
            .filter_map(|s| Url::parse(s).ok())
            .map(|u| u[url::Position::BeforeHost..url::Position::AfterPort].to_owned())
            .collect();
        Ok(Self {
            token,
            cookie: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            origins,
            hosts,
        })
    }
    fn authenticated(&self, headers: &axum::http::HeaderMap) -> bool {
        let Some(token) = &self.token else {
            return true;
        };
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|v| equal(v, token))
        {
            return true;
        }
        headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';').any(|part| {
                    part.trim()
                        .strip_prefix("agentdock_session=")
                        .is_some_and(|s| equal(s, &self.cookie))
                })
            })
    }
}
fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |n, (x, y)| n | (x ^ y)) == 0
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error":message}))).into_response()
}

pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if !headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| state.security.hosts.contains(v))
    {
        return error(StatusCode::FORBIDDEN, "Untrusted Host header");
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && !origin
            .to_str()
            .ok()
            .is_some_and(|v| state.security.origins.contains(v))
    {
        return error(StatusCode::FORBIDDEN, "Untrusted Origin");
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return error(StatusCode::FORBIDDEN, "Cross-site requests are blocked");
    }
    let path = request.uri().path();
    if path.starts_with("/api/") {
        let public = path == "/api/health" || path == "/api/auth";
        if !public && !state.security.authenticated(headers) {
            return error(StatusCode::UNAUTHORIZED, "Authentication required");
        }
        if !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        ) && headers.get("x-agentdock-client").is_none_or(|v| v != "web")
        {
            return error(
                StatusCode::FORBIDDEN,
                "X-AgentDock-Client: web is required for mutations",
            );
        }
        // Browser WebSocket requests always carry Origin; command-line clients may use bearer.
        if headers
            .get(header::UPGRADE)
            .is_some_and(|v| v == "websocket")
            && headers.get(header::ORIGIN).is_none()
            && state.security.token.is_some()
            && headers.get(header::AUTHORIZATION).is_none()
        {
            return error(
                StatusCode::FORBIDDEN,
                "WebSocket Origin or bearer authentication required",
            );
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        "same-origin".parse().unwrap(),
    );
    response
}

pub async fn status(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"required":state.security.token.is_some(),"authenticated":state.security.authenticated(&headers)}),
    )
}
#[derive(Deserialize)]
pub struct Login {
    token: String,
}
pub async fn login(State(state): State<AppState>, Json(input): Json<Login>) -> Response {
    if state
        .security
        .token
        .as_ref()
        .is_some_and(|t| !equal(t, &input.token))
    {
        return error(StatusCode::UNAUTHORIZED, "Invalid access token");
    }
    let secure = if state
        .security
        .origins
        .iter()
        .any(|v| v.starts_with("https://"))
    {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "agentdock_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200{secure}",
        state.security.cookie
    );
    (
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({"authenticated":true})),
    )
        .into_response()
}

#[cfg(test)]
impl Security {
    pub fn for_test(address: SocketAddr, token: Option<&str>) -> Self {
        Self {
            token: token.map(str::to_owned),
            cookie: "test-cookie".into(),
            origins: [format!("http://{address}"), "http://127.0.0.1:5173".into()].into(),
            hosts: [address.to_string(), "127.0.0.1:5173".into()].into(),
        }
    }
}
