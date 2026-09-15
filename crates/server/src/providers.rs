//! Provider launch configuration. The official CLI owns tools and approvals.
use crate::{ApiError, AppState};
use agentdock_domain::{EndpointProfile, ProviderKind, Session};
use agentdock_runtime::SpawnSpec;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use url::Url;

/// Levels both native clients accept. AgentDock forwards one of these or none.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub fn validate_effort(value: Option<&str>) -> Result<(), ApiError> {
    if value.is_some_and(|value| !EFFORT_LEVELS.contains(&value)) {
        return Err(ApiError::bad(
            "Reasoning effort must be low, medium, high, xhigh or max",
        ));
    }
    Ok(())
}

pub fn validate_profile(p: &EndpointProfile) -> Result<(), ApiError> {
    crate::environment::validate(&p.environment)?;
    if p.name.trim().is_empty() || p.name.len() > 120 {
        return Err(ApiError::bad("Profile name must be 1–120 characters"));
    }
    if matches!(p.provider, ProviderKind::Terminal) {
        return Err(ApiError::bad(
            "Terminal sessions do not use endpoint profiles",
        ));
    }
    if let Some(reference) = &p.native_config {
        // Where the requests go and who they authenticate as belong to the
        // native configuration; AgentDock must not redirect either.
        if reference.source_id.is_empty()
            || !Path::new(&reference.config_dir).is_absolute()
            || p.permission_mode != "native"
            || p.endpoint_url.is_some()
            || p.secret_ref.is_some()
        {
            return Err(ApiError::bad(
                "An existing native configuration cannot be combined with endpoint, credential, proxy or permission overrides",
            ));
        }
        // A proxy is how the account reaches its own endpoint, not a different
        // endpoint, so an official account may carry one. Account settings
        // write it here, and forbidding it left those profiles unvalidatable.
        if let Some(proxy) = p.proxy_url.as_deref() {
            validate_proxy(proxy)?;
        }
        // Mapping an official slot onto another upstream model is something both
        // clients already do themselves — Claude Code through its
        // ANTHROPIC_DEFAULT_*_MODEL variables, Codex through model_providers.
        // A second alias table here would compete with theirs and would apply
        // only to sessions AgentDock launched, so it stays refused.
        if !p.model_aliases.is_empty() {
            return Err(ApiError::bad(
                "An official account maps models through its own client configuration, not through aliases here",
            ));
        }
        // A model and a depth are launch choices, not a redirection: the client
        // still resolves and validates them against its own account.
        validate_effort(p.effort.as_deref())?;
        if let Some(model) = p.model.as_deref()
            && (model.trim().is_empty() || model.len() > 200 || model.chars().any(char::is_control))
        {
            return Err(ApiError::bad("Model must be a real model ID"));
        }
        return Ok(());
    }
    if p.provider == ProviderKind::Codex && p.permission_mode == "plan" {
        return Err(ApiError::bad(
            "Plan mode is provided by Claude Code; Codex keeps its native approval model",
        ));
    }
    if !matches!(
        p.permission_mode.as_str(),
        "native" | "interactive" | "trusted" | "plan" | "blocked"
    ) {
        return Err(ApiError::bad("Invalid permission mode"));
    }
    if let Some(value) = &p.endpoint_url {
        let url = Url::parse(value)
            .map_err(|_| ApiError::bad("Endpoint must be an absolute HTTP(S) URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiError::bad(
                "Endpoint must use HTTP(S), without embedded credentials, query or fragment",
            ));
        }
    }
    if let Some(value) = &p.proxy_url {
        validate_proxy(value)?;
    }
    // Reasoning effort is a fixed vocabulary both clients share. Anything else
    // would be forwarded to a client that would simply reject it.
    validate_effort(p.effort.as_deref())?;
    if p.model_aliases.len() > 100
        || p.model_aliases.iter().any(|(alias, id)| {
            alias.trim().is_empty()
                || id.trim().is_empty()
                || alias.len() > 120
                || id.len() > 200
                || alias.chars().any(char::is_control)
                || id.chars().any(char::is_control)
        })
    {
        return Err(ApiError::bad(
            "Model aliases must map a short name to a real model ID (maximum 100 mappings)",
        ));
    }
    if p.model
        .as_ref()
        .is_some_and(|s| s.len() > 200 || s.chars().any(char::is_control))
    {
        return Err(ApiError::bad("Invalid model identifier"));
    }
    if let Some(reference) = &p.secret_ref {
        let source = reference
            .strip_prefix("env:")
            .ok_or_else(|| ApiError::bad("Use env:VARIABLE_NAME; never enter a secret value"))?;
        if !source.starts_with("AGENTDOCK_SECRET_")
            || !source
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(ApiError::bad(
                "Secret references must use env:AGENTDOCK_SECRET_NAME",
            ));
        }
    }
    Ok(())
}

pub fn private_dir(path: &Path) -> Result<PathBuf, ApiError> {
    fs::create_dir_all(path).map_err(ApiError::internal)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(ApiError::internal)?;
    }
    fs::canonicalize(path).map_err(ApiError::internal)
}

pub fn validate_proxy(value: &str) -> Result<(), ApiError> {
    let url = Url::parse(value).map_err(|_| ApiError::bad("Proxy must be an HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return Err(ApiError::bad(
            "Use an HTTP(S) proxy without credentials or path. Claude Code does not support SOCKS.",
        ));
    }
    Ok(())
}

pub fn resolve_model(profile: &EndpointProfile) -> Option<String> {
    profile
        .model
        .as_ref()
        .map(|model| profile.model_aliases.get(model).unwrap_or(model).clone())
}

fn write_private(path: &Path, content: &str) -> Result<(), ApiError> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => file
            .write_all(content.as_bytes())
            .map_err(ApiError::internal),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(ApiError::internal(e)),
    }
}

fn encode(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization")
}

/// No profile/snapshot means native defaults in a fresh, persistent config dir.
pub fn build(state: &AppState, session: &Session, cwd: PathBuf) -> Result<SpawnSpec, ApiError> {
    crate::environment::validate(&session.environment)?;
    let mut spec = build_base(state, session, cwd)?;
    // Claude Code owns the native session title. Pass the AgentDock title on
    // every new/reopened Claude process; renaming a live process remains a
    // display-only change until its next explicit reopen.
    if session.provider == ProviderKind::ClaudeCode
        && session.native_source_id.is_none()
        && session
            .endpoint_snapshot
            .as_ref()
            .is_none_or(|profile| profile.native_config.is_none())
        && !session.title.trim().is_empty()
    {
        spec.args.extend(["--name".into(), session.title.clone()]);
    }
    crate::environment::apply(&mut spec, &session.environment)?;
    Ok(spec)
}

fn build_base(state: &AppState, session: &Session, cwd: PathBuf) -> Result<SpawnSpec, ApiError> {
    if session.native_source_id.is_some() {
        return crate::native_history::resume_spec(state, session, cwd);
    }
    if let Some(profile) = session.endpoint_snapshot.as_ref()
        && let Some(reference) = profile.native_config.as_ref()
    {
        validate_profile(profile)?;
        if profile.provider != session.provider {
            return Err(ApiError::bad("Profile and session providers must match"));
        }
        let mut spec = crate::native_config::build(state, &session.provider, reference, cwd)?;
        // An official account may still choose a model and a depth. The client
        // resolves both against its own account; this only states the choice at
        // launch, which is also the one path that works for a model whose live
        // switch the upstream refuses.
        if let Some(model) = resolve_model(profile) {
            spec.args.extend(["--model".into(), model]);
        }
        if let Some(effort) = profile.effort.as_deref() {
            match session.provider {
                ProviderKind::Codex => spec.args.extend([
                    "-c".into(),
                    format!("model_reasoning_effort={}", encode(effort)),
                ]),
                _ => spec.args.extend(["--effort".into(), effort.to_owned()]),
            }
        }
        return Ok(spec);
    }
    let mut environment = BTreeMap::new();
    let mut args = Vec::new();
    let p = session.endpoint_snapshot.as_ref();
    if let Some(p) = p {
        validate_profile(p)?;
        if p.permission_mode == "blocked" {
            return Err(ApiError::bad("This profile blocks session launch"));
        }
    }
    let mut remove: Vec<String> = env::vars()
        .filter_map(|(key, _)| {
            (key.starts_with("AGENTDOCK_SECRET_")
                || key == "AGENTDOCK_TOKEN"
                || key == "CLAUDECODE")
                .then_some(key)
        })
        .collect();
    let (program, config_key) = match session.provider {
        ProviderKind::Terminal => (
            env::var("AGENTDOCK_SHELL")
                .or_else(|_| env::var("SHELL"))
                .unwrap_or_else(|_| "/bin/sh".into()),
            None,
        ),
        ProviderKind::ClaudeCode => (
            crate::clients::program(&state.state_dir, &session.provider),
            Some("CLAUDE_CONFIG_DIR"),
        ),
        ProviderKind::Codex => (
            crate::clients::program(&state.state_dir, &session.provider),
            Some("CODEX_HOME"),
        ),
    };
    if matches!(session.provider, ProviderKind::Terminal) {
        args.push("-i".into());
    }
    if let Some(config_key) = config_key {
        let base = state
            .state_dir
            .join("sessions")
            .join(session.id.to_string());
        let path = if session.configuration_revision == 0 {
            base
        } else {
            base.join("configurations")
                .join(session.configuration_revision.to_string())
        };
        let dir = private_dir(&path)?;
        environment.insert(config_key.into(), dir.to_string_lossy().into_owned());
        // Avoid inherited endpoint/auth overrides from the AgentDock host.
        remove.extend(
            [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
                "CODEX_API_KEY",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let secret = if let Some(reference) = p.and_then(|p| p.secret_ref.as_deref()) {
            let source = reference
                .strip_prefix("env:")
                .ok_or_else(|| ApiError::bad("Invalid secret reference"))?;
            Some(env::var(source).map_err(|_| {
                ApiError::bad(format!(
                    "Secret reference {source} is not configured on the server"
                ))
            })?)
        } else {
            None
        };
        if let Some(model) = p.and_then(resolve_model) {
            args.extend(["--model".into(), model]);
        }
        if let Some(proxy) = p.and_then(|p| p.proxy_url.as_ref()) {
            // Override both cases so an inherited lowercase variable cannot win.
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                environment.insert(key.into(), proxy.clone());
            }
            for key in ["NO_PROXY", "no_proxy"] {
                environment.insert(key.into(), String::new());
            }
        }
        let mode = p.map_or("native", |p| p.permission_mode.as_str());
        match session.provider {
            ProviderKind::ClaudeCode => {
                if let Some(url) = p.and_then(|p| p.endpoint_url.as_ref()) {
                    environment.insert("ANTHROPIC_BASE_URL".into(), url.clone());
                }
                if let Some(secret) = secret {
                    environment.insert("ANTHROPIC_API_KEY".into(), secret);
                }
                if mode == "trusted" {
                    args.extend(["--permission-mode".into(), "acceptEdits".into()]);
                }
                if mode == "plan" {
                    args.extend(["--permission-mode".into(), "plan".into()]);
                }
                // Current CLI uses "manual"; older clients called it "default".
                // Native mode omits this flag entirely.
                if mode == "interactive" {
                    let manual = state.claude_manual_mode;
                    args.extend([
                        "--permission-mode".into(),
                        if manual { "manual" } else { "default" }.into(),
                    ]);
                }
                if let Some(effort) = p.and_then(|p| p.effort.as_deref()) {
                    args.extend(["--effort".into(), effort.to_owned()]);
                }
                // Record this session's own usage payload locally so the account
                // page can answer an explicit usage check later.
                let config_env = environment.get("CLAUDE_CONFIG_DIR").cloned();
                let (overlay, capture) =
                    crate::native_config::claude_statusline(state, config_env.as_deref());
                args.extend(overlay);
                environment.extend(capture);
            }
            ProviderKind::Codex => {
                let mut config = "cli_auth_credentials_store = \"file\"\n".to_owned();
                if mode == "interactive" || mode == "trusted" {
                    // Same sandbox for both intents; no bypass or fictitious mapping.
                    args.extend([
                        "-c".into(),
                        "approval_policy=\"on-request\"".into(),
                        "-c".into(),
                        "sandbox_mode=\"workspace-write\"".into(),
                    ]);
                }
                if let Some(effort) = p.and_then(|p| p.effort.as_deref()) {
                    args.extend([
                        "-c".into(),
                        format!("model_reasoning_effort={}", encode(effort)),
                    ]);
                }
                if p.is_some_and(|p| p.endpoint_url.is_some() || p.secret_ref.is_some()) {
                    config.push_str("model_provider = \"agentdock\"\n\n[model_providers.agentdock]\nname = \"AgentDock\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n");
                    let base = p
                        .and_then(|p| p.endpoint_url.as_deref())
                        .unwrap_or("https://api.openai.com/v1");
                    config.push_str(&format!("base_url = {}\n", encode(base)));
                    if let Some(secret) = secret {
                        config.push_str("env_key = \"OPENAI_API_KEY\"\n");
                        environment.insert("OPENAI_API_KEY".into(), secret);
                    }
                }
                // Immutable generated config. CLI auth/history/settings remain its own.
                write_private(&dir.join("config.toml"), &config)?;
            }
            ProviderKind::Terminal => {}
        }
    }
    // Runtime removes inherited vars first, then injects only this session's config.
    remove.retain(|key| !environment.contains_key(key));
    Ok(SpawnSpec {
        program,
        args,
        cwd,
        env: environment,
        env_remove: remove,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_rejects_secret_values_and_embedded_auth() {
        let mut p = EndpointProfile {
            id: uuid::Uuid::new_v4(),
            name: "one".into(),
            provider: ProviderKind::Codex,
            endpoint_url: Some("https://name:secret@example.test".into()),
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: None,
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        };
        assert!(validate_profile(&p).is_err());
        p.endpoint_url = Some("https://example.test/v1".into());
        p.secret_ref = Some("env:HOME".into());
        assert!(validate_profile(&p).is_err());
        p.secret_ref = Some("env:AGENTDOCK_SECRET_WORK".into());
        assert!(validate_profile(&p).is_ok());
    }

    #[test]
    fn an_official_account_may_choose_a_model_but_never_where_requests_go() {
        let mut p = EndpointProfile {
            id: uuid::Uuid::new_v4(),
            name: "official".into(),
            provider: ProviderKind::ClaudeCode,
            endpoint_url: None,
            model: None,
            permission_mode: "native".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: Some(agentdock_domain::NativeConfigReference {
                source_id: "claude-default".into(),
                config_dir: "/tmp/agentdock-fixture".into(),
                config_env: None,
            }),
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        };
        assert!(validate_profile(&p).is_ok());
        // A model and a depth are launch choices the client still resolves
        // against its own account, so an official account may carry them.
        p.model = Some("claude-fable-5-1[1m]".into());
        p.effort = Some("high".into());
        assert!(validate_profile(&p).is_ok());
        // The account writes its proxy here; refusing it left those profiles
        // stored but unvalidatable.
        p.proxy_url = Some("http://192.168.0.253:7890".into());
        assert!(validate_profile(&p).is_ok());
        p.proxy_url = Some("http://user:pass@proxy.test".into());
        assert!(validate_profile(&p).is_err());
        p.proxy_url = None;
        // Where the requests go and who they authenticate as stay the native
        // configuration's, and a second alias table would compete with the
        // client's own mapping.
        p.model_aliases = [("fast".to_string(), "haiku".to_string())].into();
        assert!(validate_profile(&p).is_err());
        p.model_aliases = Default::default();
        p.endpoint_url = Some("https://relay.test/v1".into());
        assert!(validate_profile(&p).is_err());
        p.endpoint_url = None;
        p.secret_ref = Some("env:AGENTDOCK_SECRET_WORK".into());
        assert!(validate_profile(&p).is_err());
    }

    #[test]
    fn plan_is_forwarded_only_to_claude_code() {
        let mut p = EndpointProfile {
            id: uuid::Uuid::new_v4(),
            name: "plan".into(),
            provider: ProviderKind::ClaudeCode,
            endpoint_url: None,
            model: None,
            permission_mode: "plan".into(),
            secret_ref: None,
            proxy_url: None,
            effort: None,
            model_aliases: Default::default(),
            native_config: None,
            environment: Default::default(),
            created_at: chrono::Utc::now(),
        };
        assert!(validate_profile(&p).is_ok());
        p.provider = ProviderKind::Codex;
        assert!(validate_profile(&p).is_err());
    }
}
