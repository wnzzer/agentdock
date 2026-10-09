//! Explicit overrides only. No host environment snapshot is persisted or exposed.
use crate::ApiError;
use agentdock_domain::{EnvironmentOverrides, EnvironmentValue};
use agentdock_runtime::SpawnSpec;

pub fn validate(values: &EnvironmentOverrides) -> Result<(), ApiError> {
    agentdock_domain::validate_environment(values).map_err(ApiError::bad)
}

/// What a launch knows that an environment value can follow: the session's
/// main model, and the context window the user set for it.
#[derive(Default, Clone, Copy)]
pub struct Launch<'a> {
    pub main_model: Option<&'a str>,
    pub main_context: Option<u64>,
}

/// Secret references resolve from the server's environment, then from the
/// secrets AgentDock stores (see secrets.rs); a main-model value becomes the
/// session's model at this launch, and a main-model-context value its window.
pub fn apply(
    spec: &mut SpawnSpec,
    values: &EnvironmentOverrides,
    state_dir: &std::path::Path,
    launch: Launch<'_>,
) -> Result<(), ApiError> {
    apply_with(spec, values, launch, |key| {
        crate::secrets::resolve(state_dir, key)
    })
}

fn apply_with(
    spec: &mut SpawnSpec,
    values: &EnvironmentOverrides,
    launch: Launch<'_>,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Result<(), ApiError> {
    validate(values)?;
    for (key, value) in values {
        let resolved = match value {
            EnvironmentValue::Literal { value } => Some(value.clone()),
            EnvironmentValue::SecretRef { reference } => {
                let name = reference.strip_prefix("env:").expect("validated reference");
                let value = resolve(name).ok_or_else(|| {
                    ApiError::bad(format!(
                        "Secret reference for {key} is not configured on the server"
                    ))
                })?;
                if value.len() > 8192 || value.contains('\0') {
                    return Err(ApiError::bad(format!(
                        "Resolved environment value for {key} exceeds 8192 bytes or contains NUL"
                    )));
                }
                Some(value)
            }
            EnvironmentValue::Unset => None,
            // No main model: the variable is left out, so the client keeps
            // its own default for that slot.
            EnvironmentValue::MainModel => match launch.main_model {
                Some(model) => Some(model.to_owned()),
                None => continue,
            },
            EnvironmentValue::MainModelContext => match launch.main_context {
                Some(window) => Some(window.to_string()),
                None => continue,
            },
        };
        if let Some(value) = resolved {
            spec.env_remove.retain(|removed| removed != key);
            spec.env.insert(key.clone(), value);
        } else {
            spec.env.remove(key);
            if !spec.env_remove.contains(key) {
                spec.env_remove.push(key.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_overrides_merge_without_snapshotting_or_modifying_parent_environment() {
        let mut spec = SpawnSpec {
            program: "fixture".into(),
            args: vec![],
            cwd: "/fixture".into(),
            env: [
                ("KEEP".into(), "base".into()),
                ("REMOVE".into(), "old".into()),
            ]
            .into(),
            env_remove: vec!["RESTORE".into(), "OPENAI_API_KEY".into(), "OTHER".into()],
        };
        let values = [
            (
                "RESTORE".into(),
                EnvironmentValue::Literal {
                    value: "explicit".into(),
                },
            ),
            (
                "OPENAI_API_KEY".into(),
                EnvironmentValue::SecretRef {
                    reference: "env:AGENTDOCK_SECRET_FIXTURE".into(),
                },
            ),
            ("REMOVE".into(), EnvironmentValue::Unset),
        ]
        .into();
        apply_with(&mut spec, &values, Launch::default(), |key| {
            assert_eq!(key, "AGENTDOCK_SECRET_FIXTURE");
            Some("synthetic-secret-value".into())
        })
        .unwrap();
        assert_eq!(spec.env["KEEP"], "base");
        assert_eq!(spec.env["RESTORE"], "explicit");
        assert_eq!(spec.env["OPENAI_API_KEY"], "synthetic-secret-value");
        assert!(!spec.env.contains_key("REMOVE"));
        assert_eq!(spec.env_remove, vec!["OTHER", "REMOVE"]);
        assert!(
            !serde_json::to_string(&values)
                .unwrap()
                .contains("synthetic-secret-value")
        );
        assert!(!format!("{values:?}").contains("explicit"));
        assert!(!format!("{spec:?}").contains("synthetic-secret-value"));
    }
    #[test]
    fn secret_resolution_errors_do_not_disclose_values() {
        let mut spec = SpawnSpec {
            program: "fixture".into(),
            args: vec![],
            cwd: "/fixture".into(),
            env: Default::default(),
            env_remove: vec![],
        };
        let values = [(
            "OPENAI_API_KEY".into(),
            EnvironmentValue::SecretRef {
                reference: "env:AGENTDOCK_SECRET_FIXTURE".into(),
            },
        )]
        .into();
        assert!(apply_with(&mut spec, &values, Launch::default(), |_| None).is_err());
        let error = apply_with(&mut spec, &values, Launch::default(), |_| {
            Some("secret\0suffix".into())
        })
        .unwrap_err();
        assert!(!error.message.contains("secret\0suffix"));
    }

    #[test]
    fn a_main_model_value_follows_the_sessions_model_or_is_left_out() {
        let values: EnvironmentOverrides = [(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned(),
            EnvironmentValue::MainModel,
        )]
        .into();
        let mut spec = SpawnSpec {
            program: "fixture".into(),
            args: vec![],
            cwd: "/fixture".into(),
            env: Default::default(),
            env_remove: vec![],
        };
        apply_with(
            &mut spec,
            &values,
            Launch {
                main_model: Some("qwen-gateway-id"),
                main_context: None,
            },
            |_| None,
        )
        .unwrap();
        assert_eq!(
            spec.env
                .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .map(String::as_str),
            Some("qwen-gateway-id")
        );
        let mut spec = SpawnSpec {
            program: "fixture".into(),
            args: vec![],
            cwd: "/fixture".into(),
            env: Default::default(),
            env_remove: vec![],
        };
        apply_with(&mut spec, &values, Launch::default(), |_| None).unwrap();
        assert!(!spec.env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"));
        assert!(
            !spec
                .env_remove
                .contains(&"ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned()),
            "no main model leaves the client its own default, not a removal"
        );
        let json = serde_json::to_value(&values).unwrap();
        assert_eq!(json["ANTHROPIC_DEFAULT_HAIKU_MODEL"]["kind"], "main_model");
    }

    #[test]
    fn a_main_model_context_value_is_the_window_set_for_that_model() {
        let values: EnvironmentOverrides = [(
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS".to_owned(),
            EnvironmentValue::MainModelContext,
        )]
        .into();
        let spec = || SpawnSpec {
            program: "fixture".into(),
            args: vec![],
            cwd: "/fixture".into(),
            env: Default::default(),
            env_remove: vec![],
        };
        let mut set = spec();
        let launch = Launch {
            main_model: Some("qwen-gateway-id"),
            main_context: Some(32_768),
        };
        apply_with(&mut set, &values, launch, |_| None).unwrap();
        assert_eq!(
            set.env
                .get("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
                .map(String::as_str),
            Some("32768")
        );
        let mut unset = spec();
        apply_with(&mut unset, &values, Launch::default(), |_| None).unwrap();
        assert!(
            !unset.env.contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            "no window set: Claude Code keeps its own"
        );
    }
}
