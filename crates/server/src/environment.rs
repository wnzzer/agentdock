//! Explicit overrides only. No host environment snapshot is persisted or exposed.
use crate::ApiError;
use agentdock_domain::{EnvironmentOverrides, EnvironmentValue};
use agentdock_runtime::SpawnSpec;

pub fn validate(values: &EnvironmentOverrides) -> Result<(), ApiError> {
    agentdock_domain::validate_environment(values).map_err(ApiError::bad)
}

pub fn apply(spec: &mut SpawnSpec, values: &EnvironmentOverrides) -> Result<(), ApiError> {
    apply_with(spec, values, |key| std::env::var(key).ok())
}

fn apply_with(
    spec: &mut SpawnSpec,
    values: &EnvironmentOverrides,
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
        apply_with(&mut spec, &values, |key| {
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
        assert!(apply_with(&mut spec, &values, |_| None).is_err());
        let error = apply_with(&mut spec, &values, |_| Some("secret\0suffix".into())).unwrap_err();
        assert!(!error.message.contains("secret\0suffix"));
    }
}
