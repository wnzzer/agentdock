//! Starting the Node bridges, and what no child of this server may inherit.
//!
//! Every bridge script (chat, accounts, history, file search) runs the same
//! way: under the configured JavaScript runtime, in a process group of its own,
//! with piped stdio, and without AgentDock's own secrets in its environment.
//! Keeping that in one place is what keeps the four from drifting apart, and
//! [`is_agentdock_secret`] is the one definition every launch path filters by.
use agentdock_runtime::process;
use std::{
    env,
    ffi::{OsStr, OsString},
    path::Path,
    process::Stdio,
};

/// A variable that holds AgentDock's own credentials: endpoint secrets
/// referenced as `env:AGENTDOCK_SECRET_*`, and the gateway's access token.
/// These never reach a bridge, a native client or a terminal.
pub fn is_agentdock_secret(key: &str) -> bool {
    key.starts_with("AGENTDOCK_SECRET_") || key == "AGENTDOCK_TOKEN"
}

/// The names of AgentDock's secrets present in this process's environment.
pub fn agentdock_secrets() -> impl Iterator<Item = String> {
    env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| is_agentdock_secret(key))
}

/// The JavaScript runtime the bridges run under: `AGENTDOCK_JS_RUNTIME`, then
/// the legacy `AGENTDOCK_NODE_BIN`, then `node`. An empty value is unset.
pub fn js_runtime() -> OsString {
    choose_runtime(
        env::var_os("AGENTDOCK_JS_RUNTIME"),
        env::var_os("AGENTDOCK_NODE_BIN"),
    )
}

fn choose_runtime(primary: Option<OsString>, legacy: Option<OsString>) -> OsString {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| legacy.filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "node".into())
}

/// A command that runs `script` under [`js_runtime`], with `node_options`
/// before the script. Its stdin and stdout are piped and stderr discarded, it
/// is killed if dropped, it leads a process group of its own (so the caller
/// can stop it and everything it starts with an [`process::OwnedGroup`]), and
/// AgentDock's secrets are removed from its environment.
pub fn node_command(node_options: &[&str], script: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(js_runtime());
    command
        .args(node_options)
        .arg(script.as_ref())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in agentdock_secrets() {
        command.env_remove(key);
    }
    process::own_group(command.as_std_mut());
    command
}

/// The bridge script `name` beside the installed native bridge, unless the
/// environment variable `override_key` names another.
pub fn script(state_dir: &Path, override_key: &str, name: &str) -> std::path::PathBuf {
    env::var_os(override_key)
        .map(Into::into)
        .unwrap_or_else(|| crate::installation::native_bridge(state_dir).with_file_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_runtime_defaults_to_node_and_respects_both_overrides() {
        assert_eq!(choose_runtime(None, None), OsString::from("node"));
        assert_eq!(
            choose_runtime(Some("/opt/runtime/bun".into()), Some("node".into())),
            OsString::from("/opt/runtime/bun")
        );
        assert_eq!(
            choose_runtime(None, Some("/legacy/node".into())),
            OsString::from("/legacy/node")
        );
        assert_eq!(
            choose_runtime(Some(OsString::new()), Some("legacy-node".into())),
            OsString::from("legacy-node")
        );
        assert_eq!(
            choose_runtime(Some(OsString::new()), Some(OsString::new())),
            OsString::from("node")
        );
    }

    #[test]
    fn only_agentdock_credentials_count_as_its_secrets() {
        for key in [
            "AGENTDOCK_SECRET_WORK",
            "AGENTDOCK_SECRET_",
            "AGENTDOCK_TOKEN",
        ] {
            assert!(is_agentdock_secret(key), "{key}");
        }
        // Configuration is not a secret, and neither is a lookalike.
        for key in [
            "AGENTDOCK_HOME",
            "AGENTDOCK_TOKEN_FILE",
            "MY_AGENTDOCK_TOKEN",
            "OPENAI_API_KEY",
        ] {
            assert!(!is_agentdock_secret(key), "{key}");
        }
    }
}
