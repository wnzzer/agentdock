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

/// Shortest token that is worth calling one. A network-reachable workspace is a
/// shell on the machine, so a guessable password is the same as none.
pub const MINIMUM_TOKEN: usize = 24;

fn token_file(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join("token")
}

/// The access token for a binding that reaches other machines.
///
/// `AGENTDOCK_TOKEN` still wins, so an existing deployment and a secret manager
/// keep working. Otherwise one is generated and kept, which is what makes a
/// background gateway usable at all: a token printed once by a detached process
/// is a token nobody can read afterwards, and `status` has to be able to say
/// what it is.
///
/// Keeping it on disk means whoever can read the file can reach the workspace.
/// That is a smaller step than it sounds: the same reader can already open the
/// database and the client configuration directories next to it, which is the
/// account itself. The file is owner-only, and deleting it issues a new token
/// on the next start.
pub fn resolve_token(
    state_dir: &std::path::Path,
    needed: bool,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if let Some(explicit) = env::var("AGENTDOCK_TOKEN").ok().filter(|v| !v.is_empty()) {
        return Ok(Some(explicit));
    }
    if !needed {
        return Ok(None);
    }
    let path = token_file(state_dir);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_owned();
        if existing.chars().count() >= MINIMUM_TOKEN {
            return Ok(Some(existing));
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    write_owner_only(&path, &token)?;
    Ok(Some(token))
}

/// Create or replace a file only its owner can read.
///
/// Deliberately not `providers::write_private`, which refuses to overwrite: a
/// token file left too short by an earlier attempt has to be replaced, and
/// there the no-op would keep the value that was already rejected.
fn write_owner_only(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(content.as_bytes())?;
    // An existing file keeps its old mode, so state it either way.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The token a running gateway is using, for reporting it again later. Never
/// generates one: `status` describing a token the server does not have would be
/// worse than saying nothing.
pub fn stored_token(state_dir: &std::path::Path) -> Option<String> {
    env::var("AGENTDOCK_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::fs::read_to_string(token_file(state_dir))
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
}

#[derive(Clone)]
pub struct Security {
    token: Option<String>,
    cookie: String,
    origins: HashSet<String>,
    hosts: HashSet<String>,
    /// The port that was bound, so a request naming this machine by its own
    /// address can be recognised without listing the interfaces it has.
    port: u16,
}
impl Security {
    pub fn new(
        address: SocketAddr,
        token: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let token = token.filter(|v| !v.is_empty());
        if !address.ip().is_loopback() {
            // Say what was actually wrong. "Requires a token" when one was given
            // sends someone looking for a typo in the variable name rather than
            // at its length, which is what this has cost twice.
            match token.as_deref() {
                None => {
                    return Err(format!(
                        "Listening on {address} reaches other machines, so it needs an access token of at least {MINIMUM_TOKEN} characters. Set AGENTDOCK_TOKEN, or let AgentDock generate one by not setting it."
                    )
                    .into());
                }
                Some(value) if value.chars().count() < MINIMUM_TOKEN => {
                    return Err(format!(
                        "AGENTDOCK_TOKEN is {} characters; {MINIMUM_TOKEN} is the minimum for a binding that reaches other machines.",
                        value.chars().count()
                    )
                    .into());
                }
                Some(_) => {}
            }
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
            port: address.port(),
        })
    }

    /// Whether a `Host` header names this machine by an address rather than a
    /// name, on the port actually bound.
    ///
    /// The Host check exists to stop DNS rebinding, and rebinding needs a
    /// *name*: the attacker owns `evil.com` and repoints it at a private
    /// address, so the browser still sends `Host: evil.com`. A literal address
    /// in Host means the browser was pointed straight at an address, which no
    /// name the attacker controls can produce. Cross-site requests remain
    /// blocked by the Origin and `sec-fetch-site` checks, which are separate.
    ///
    /// This is what makes `AGENTDOCK_ALLOWED_ORIGINS` unnecessary for reaching
    /// a LAN address: the server already knows its own port, and requiring the
    /// address to be repeated produced a 403 that named neither.
    fn addressed_by_ip(&self, host: &str) -> bool {
        let (name, port) = match host.rsplit_once(':') {
            // A bracketed IPv6 literal without a port cannot be split this way.
            Some((name, port)) if !name.ends_with('[') => (name, port.parse::<u16>().ok()),
            _ => (host, None),
        };
        if port != Some(self.port) {
            return false;
        }
        let name = name.trim_start_matches('[').trim_end_matches(']');
        name.parse::<std::net::IpAddr>().is_ok()
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
        .is_some_and(|v| state.security.hosts.contains(v) || state.security.addressed_by_ip(v))
    {
        return error(
            StatusCode::FORBIDDEN,
            "This address is not one AgentDock was told to answer for. Add it to AGENTDOCK_ALLOWED_ORIGINS.",
        );
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
            port: address.port(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(port: u16) -> Security {
        Security::for_test(format!("127.0.0.1:{port}").parse().unwrap(), None)
    }

    /// Reaching the workspace at the machine's own address must not require
    /// that address to have been configured in advance. The 403 that produced
    /// named neither the address it wanted nor the variable that supplies it.
    #[test]
    fn an_address_on_the_bound_port_is_answered_for_without_being_configured() {
        let security = bound(28789);
        for host in [
            "192.168.0.252:28789",
            "10.0.0.5:28789",
            "127.0.0.1:28789",
            "[::1]:28789",
            "[fe80::1]:28789",
        ] {
            assert!(security.addressed_by_ip(host), "{host}");
        }
    }

    /// A name is exactly what DNS rebinding needs, so a name is what the
    /// configured list stays responsible for.
    #[test]
    fn a_hostname_is_never_answered_for_merely_because_the_port_matches() {
        let security = bound(28789);
        for host in [
            "evil.com:28789",
            "agentdock.local:28789",
            "192.168.0.252.evil.com:28789",
            "localhost:28789",
        ] {
            assert!(!security.addressed_by_ip(host), "{host}");
        }
    }

    /// Another service on the same machine is a different server. Answering for
    /// its port would let a page served there drive this one.
    #[test]
    fn an_address_on_a_different_port_is_not_this_server() {
        let security = bound(28789);
        for host in [
            "192.168.0.252:28790",
            "192.168.0.252",
            "192.168.0.252:",
            "192.168.0.252:notaport",
        ] {
            assert!(!security.addressed_by_ip(host), "{host}");
        }
    }

    /// The length rule is the whole protection for a network binding, so it is
    /// stated here rather than left to whoever reads the constructor.
    /// `Security` holds the token, so it deliberately has no `Debug`; reading
    /// the rejection means matching rather than `unwrap_err`.
    fn rejection(address: SocketAddr, token: Option<String>) -> String {
        match Security::new(address, token) {
            Ok(_) => String::new(),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn a_binding_that_reaches_other_machines_refuses_a_short_or_absent_token() {
        let network: SocketAddr = "0.0.0.0:28789".parse().unwrap();
        let absent = rejection(network, None);
        assert!(absent.contains("at least 24"), "{absent}");
        // Saying how long it was is the difference between looking at the
        // value and looking for a typo in the variable name.
        let short = rejection(network, Some("qwe41235".into()));
        assert!(short.contains("is 8 characters"), "{short}");
        assert!(rejection(network, Some("x".repeat(24))).is_empty());
    }

    /// Loopback keeps working with no token at all: the single-user host case
    /// is the common one and must not acquire a password.
    #[test]
    fn a_loopback_binding_still_needs_nothing() {
        assert!(rejection("127.0.0.1:28789".parse().unwrap(), None).is_empty());
    }
}
