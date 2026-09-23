use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type EnvironmentOverrides = std::collections::BTreeMap<String, EnvironmentValue>;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentValue {
    Literal { value: String },
    SecretRef { reference: String },
    Unset,
}

impl std::fmt::Debug for EnvironmentValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Literal { .. } => "Literal { value: [REDACTED] }",
            Self::SecretRef { .. } => "SecretRef { reference: [REDACTED] }",
            Self::Unset => "Unset",
        })
    }
}

/// Validate explicit overrides without inspecting the host environment.
pub fn validate_environment(environment: &EnvironmentOverrides) -> Result<(), String> {
    if environment.len() > 64
        || serde_json::to_vec(environment)
            .map_err(|_| "Invalid environment")?
            .len()
            > 64 * 1024
    {
        return Err("Environment supports at most 64 entries and 64 KiB of JSON".into());
    }
    for (key, value) in environment {
        let mut bytes = key.bytes();
        if key.len() > 128
            || !bytes
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("Environment keys must match [A-Za-z_][A-Za-z0-9_]{0,127}".into());
        }
        let upper = key.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "HOME"
                | "USERPROFILE"
                | "PWD"
                | "OLDPWD"
                | "CODEX_HOME"
                | "CLAUDE_CONFIG_DIR"
                | "CLAUDECODE"
        ) || upper.starts_with("AGENTDOCK_")
        {
            return Err(format!("Environment key {key} is reserved"));
        }
        match value {
            EnvironmentValue::Literal { value } => {
                if value.len() > 8192 || value.contains('\0') {
                    return Err(format!(
                        "Environment value for {key} exceeds 8192 bytes or contains NUL"
                    ));
                }
                if [
                    "TOKEN",
                    "SECRET",
                    "PASSWORD",
                    "PRIVATE_KEY",
                    "API_KEY",
                    "AUTHORIZATION",
                ]
                .iter()
                .any(|marker| upper.contains(marker))
                {
                    return Err(format!(
                        "Sensitive environment key {key} requires a secret reference or unset"
                    ));
                }
                if matches!(upper.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")
                    || upper.ends_with("_BASE_URL")
                {
                    let parsed = url::Url::parse(value).or_else(|_| {
                        url::Url::parse(&format!("http://{}", value.trim_start_matches("//")))
                    });
                    let authority = value
                        .split_once("://")
                        .map_or(value.as_str(), |(_, rest)| rest)
                        .trim_start_matches("//")
                        .split(['/', '?', '#'])
                        .next()
                        .unwrap_or("");
                    if authority.contains('@')
                        || parsed
                            .is_ok_and(|url| !url.username().is_empty() || url.password().is_some())
                    {
                        return Err(format!(
                            "Environment URL for {key} contains credentials; use a secret reference"
                        ));
                    }
                }
            }
            EnvironmentValue::SecretRef { reference } => {
                if !reference
                    .strip_prefix("env:AGENTDOCK_SECRET_")
                    .is_some_and(|name| {
                        !name.is_empty()
                            && name
                                .bytes()
                                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                    })
                {
                    return Err(format!(
                        "Environment secret reference for {key} must use env:AGENTDOCK_SECRET_NAME"
                    ));
                }
            }
            EnvironmentValue::Unset => {}
        }
    }
    Ok(())
}

pub type WorkspaceId = Uuid;
pub type SessionId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub root_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    ClaudeCode,
    Codex,
    Terminal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Running,
    Waiting,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub interaction_mode: InteractionMode,
    #[serde(default)]
    pub configuration_revision: u64,
    #[serde(default)]
    pub environment: EnvironmentOverrides,
    pub id: SessionId,
    pub workspace_id: WorkspaceId,
    pub provider: ProviderKind,
    pub title: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Reversible list organization only; archiving never changes the runtime.
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    /// Declared at creation and never set afterwards, so a session that was
    /// safe to keep can never become one that closing its window destroys.
    #[serde(default)]
    pub ephemeral: bool,
    pub endpoint_profile_id: Option<Uuid>,
    pub provider_session_id: Option<String>,
    pub error: Option<String>,
    pub endpoint_snapshot: Option<EndpointProfile>,
    pub native_source_id: Option<String>,
    #[serde(skip_serializing)]
    pub native_config_dir: Option<String>,
    /// The structured session whose conversation this terminal reopens. Present
    /// only on an escape-hatch session opened from structured mode; it names the
    /// config home `--resume` must read, which belongs to that source session.
    #[serde(default)]
    pub resume_source_id: Option<SessionId>,
    /// A worktree of the workspace's repository this session runs in, instead
    /// of the workspace directory; `None` is the workspace itself.
    #[serde(default)]
    pub checkout_path: Option<String>,
    /// The branch that checkout was on when it was chosen, for labels.
    #[serde(default)]
    pub checkout_branch: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    #[default]
    Pty,
    Structured,
}

/// A reference to configuration owned by an already-installed native client.
/// This contains only source identity/path, never configuration or credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeConfigReference {
    pub source_id: String,
    pub config_dir: String,
    /// Original native directory environment value, without canonicalization.
    /// None preserves the client's default (unset) credential/Keychain context.
    #[serde(default)]
    pub config_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointProfile {
    #[serde(default)]
    pub environment: EnvironmentOverrides,
    pub id: Uuid,
    pub name: String,
    pub provider: ProviderKind,
    pub endpoint_url: Option<String>,
    pub model: Option<String>,
    pub permission_mode: String,
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// Reasoning effort default for this profile. Applied only when the client
    /// reports the selected model supports that level.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub model_aliases: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub native_config: Option<NativeConfigReference>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Native,
    Interactive,
    Trusted,
    Blocked,
}

#[cfg(test)]
mod environment_tests {
    use super::*;
    fn literal(key: &str, value: &str) -> EnvironmentOverrides {
        [(
            key.into(),
            EnvironmentValue::Literal {
                value: value.into(),
            },
        )]
        .into()
    }
    #[test]
    fn validates_limits_reserved_keys_and_secret_representation_without_reading_environment() {
        assert!(
            validate_environment(&literal("NODE_OPTIONS", "--max-old-space-size=4096")).is_ok()
        );
        for key in [
            "HOME",
            "home",
            "USERPROFILE",
            "PWD",
            "OLDPWD",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "CLAUDECODE",
            "AGENTDOCK_ADDR",
            "AGENTDOCK_SECRET_TARGET",
            "1KEY",
            "bad-key",
            "",
        ] {
            assert!(
                validate_environment(&literal(key, "fixture")).is_err(),
                "{key}"
            );
        }
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "PASSWORD",
            "PRIVATE_KEY",
            "AUTHORIZATION",
        ] {
            assert!(validate_environment(&literal(key, "synthetic-private-value")).is_err());
            assert!(validate_environment(&[(key.into(), EnvironmentValue::Unset)].into()).is_ok());
            assert!(
                validate_environment(
                    &[(
                        key.into(),
                        EnvironmentValue::SecretRef {
                            reference: "env:AGENTDOCK_SECRET_TEST".into()
                        }
                    )]
                    .into()
                )
                .is_ok()
            );
        }
        for reference in [
            "plaintext-secret",
            "env:HOME",
            "env:AGENTDOCK_SECRET_",
            "env:AGENTDOCK_SECRET_lower",
        ] {
            assert!(
                validate_environment(
                    &[(
                        "API_KEY".into(),
                        EnvironmentValue::SecretRef {
                            reference: reference.into()
                        }
                    )]
                    .into()
                )
                .is_err()
            );
        }
        for key in ["HTTP_PROXY", "https_proxy", "ALL_PROXY", "OPENAI_BASE_URL"] {
            assert!(
                validate_environment(&literal(key, "https://user:password@example.invalid"))
                    .is_err()
            );
            assert!(validate_environment(&literal(key, "https://example.invalid/v1")).is_ok());
        }
        assert!(validate_environment(&literal("KEY", &"x".repeat(8193))).is_err());
        assert!(validate_environment(&literal("KEY", "nul\0value")).is_err());
        let too_many = (0..65)
            .map(|i| (format!("KEY_{i}"), EnvironmentValue::Unset))
            .collect();
        assert!(validate_environment(&too_many).is_err());
        let too_large = (0..9)
            .map(|i| {
                (
                    format!("KEY_{i}"),
                    EnvironmentValue::Literal {
                        value: "x".repeat(8192),
                    },
                )
            })
            .collect();
        assert!(validate_environment(&too_large).is_err());
        assert!(!format!("{:?}", literal("KEY", "do-not-log-this")).contains("do-not-log-this"));
        assert!(
            serde_json::from_str::<EnvironmentValue>(
                r#"{"kind":"literal","value":"ok","unexpected":true}"#
            )
            .is_err()
        );
    }
}
