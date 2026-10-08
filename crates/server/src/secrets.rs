//! Endpoint secrets AgentDock keeps itself.
//!
//! A profile names its key as `env:AGENTDOCK_SECRET_NAME`. That used to mean
//! only the server's environment, so adding a key meant setting a variable and
//! restarting the server -- and nothing but a person at a shell could do it.
//! The same name now also resolves from `<state>/secrets.toml`, a 0600 file
//! written through the API, so a key typed into the browser is usable at once.
//!
//! The environment still wins, because whoever set it did so on purpose and
//! outside AgentDock. Values are never read back out through the API: it lists
//! names and where each is set, nothing more.
use crate::ApiError;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const FILE: &str = "secrets.toml";
const PREFIX: &str = "AGENTDOCK_SECRET_";
const MAX_VALUE: usize = 8192;
/// One writer at a time: a read-modify-write of the file is not atomic.
static WRITE: Mutex<()> = Mutex::new(());

fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

/// A name a profile or environment override may reference.
pub fn valid_name(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// The stored secrets. A missing file is an empty store; one that exists but
/// does not parse is an error, so that writing never replaces it unseen.
fn load(state_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match fs::read_to_string(path(state_dir)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let table = toml::from_str::<BTreeMap<String, toml::Value>>(&text)
        .map_err(|error| error.to_string())?;
    Ok(table
        .into_iter()
        .filter(|(name, _)| valid_name(name))
        .filter_map(|(name, value)| value.as_str().map(|value| (name, value.to_owned())))
        .collect())
}

/// For reading: an unreadable file resolves nothing rather than failing a launch.
fn read(state_dir: &Path) -> BTreeMap<String, String> {
    load(state_dir).unwrap_or_else(|error| {
        tracing::warn!(%error, "secrets.toml is unreadable; stored secrets are ignored");
        BTreeMap::new()
    })
}

/// For changing: refuse rather than overwrite a file that could not be read.
fn load_for_write(state_dir: &Path) -> Result<BTreeMap<String, String>, ApiError> {
    load(state_dir).map_err(|_| {
        ApiError::conflict(
            "secrets.toml in the state directory is unreadable; fix or remove it first",
        )
    })
}

fn write(state_dir: &Path, secrets: &BTreeMap<String, String>) -> Result<(), ApiError> {
    let text = toml::to_string(secrets).map_err(ApiError::internal)?;
    fs::create_dir_all(state_dir).map_err(ApiError::internal)?;
    let target = path(state_dir);
    let temporary = state_dir.join(format!(".{FILE}.{}", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(ApiError::internal)
}

/// The value behind `name`: the server's environment first, then the file.
pub fn resolve(state_dir: &Path, name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name)
        && !value.is_empty()
    {
        return Some(value);
    }
    read(state_dir).remove(name)
}

/// Resolve a profile's `env:NAME` reference.
pub fn resolve_reference(state_dir: &Path, reference: &str) -> Result<String, ApiError> {
    let name = reference
        .strip_prefix("env:")
        .ok_or_else(|| ApiError::bad("Invalid secret reference"))?;
    resolve(state_dir, name).ok_or_else(|| {
        ApiError::bad(format!(
            "Secret {name} is not set: save the key in AgentDock or set it on the server"
        ))
    })
}

pub fn store(state_dir: &Path, name: &str, value: &str) -> Result<(), ApiError> {
    if !valid_name(name) {
        return Err(ApiError::bad(
            "Secret names are AGENTDOCK_SECRET_ followed by capitals, digits or _",
        ));
    }
    if value.is_empty() || value.len() > MAX_VALUE || value.contains('\0') {
        return Err(ApiError::bad(
            "A secret must be 1 to 8192 bytes without NUL characters",
        ));
    }
    let _guard = WRITE.lock().expect("secrets lock");
    let mut secrets = load_for_write(state_dir)?;
    secrets.insert(name.to_owned(), value.to_owned());
    write(state_dir, &secrets)
}

/// Whether anything was removed from the file. A name set only in the
/// environment is not AgentDock's to remove.
pub fn remove(state_dir: &Path, name: &str) -> Result<bool, ApiError> {
    let _guard = WRITE.lock().expect("secrets lock");
    let mut secrets = load_for_write(state_dir)?;
    if secrets.remove(name).is_none() {
        return Ok(false);
    }
    write(state_dir, &secrets)?;
    Ok(true)
}

#[derive(serde::Serialize, PartialEq, Debug)]
pub struct SecretName {
    pub name: String,
    /// `environment` when the server's environment sets it (and so wins),
    /// otherwise `agentdock`.
    pub source: &'static str,
    /// Whether AgentDock keeps a value of its own under this name: the one
    /// used once the server's environment no longer sets it.
    pub stored: bool,
}

/// Every name that resolves, and from where. Never the values.
pub fn list(state_dir: &Path) -> Vec<SecretName> {
    let stored: std::collections::BTreeSet<String> = read(state_dir).into_keys().collect();
    let mut names: BTreeMap<String, &'static str> = stored
        .iter()
        .map(|name| (name.clone(), "agentdock"))
        .collect();
    for (name, value) in std::env::vars() {
        if valid_name(&name) && !value.is_empty() {
            names.insert(name, "environment");
        }
    }
    names
        .into_iter()
        .map(|(name, source)| SecretName {
            stored: stored.contains(&name),
            name,
            source,
        })
        .collect()
}

/// Keep in AgentDock the value the server's environment gives `name`, so a
/// profile that read the key from there goes on working once it is gone.
/// The value is read and written here; it never travels to the browser.
pub fn adopt(state_dir: &Path, name: &str, value: Option<String>) -> Result<SecretName, ApiError> {
    let value = value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad(format!("{name} is not set in the server's environment")))?;
    store(state_dir, name, &value)?;
    Ok(SecretName {
        name: name.to_owned(),
        source: "environment",
        stored: true,
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretValue {
    value: String,
}

pub fn routes() -> axum::Router<crate::AppState> {
    use axum::routing::{get, post, put};
    axum::Router::new()
        .route("/api/secrets", get(list_secrets))
        .route("/api/secrets/{name}", put(put_secret).delete(delete_secret))
        .route("/api/secrets/{name}/adopt", post(adopt_secret))
}

async fn list_secrets(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> axum::Json<Vec<SecretName>> {
    axum::Json(list(&state.state_dir))
}

async fn put_secret(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::Json(input): axum::Json<SecretValue>,
) -> Result<axum::Json<SecretName>, ApiError> {
    store(&state.state_dir, &name, &input.value)?;
    let environment = std::env::var(&name).is_ok_and(|value| !value.is_empty());
    Ok(axum::Json(SecretName {
        name,
        source: if environment {
            "environment"
        } else {
            "agentdock"
        },
        stored: true,
    }))
}

async fn adopt_secret(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<SecretName>, ApiError> {
    if !valid_name(&name) {
        return Err(ApiError::bad(
            "Secret names are AGENTDOCK_SECRET_ followed by capitals, digits or _",
        ));
    }
    let value = std::env::var(&name).ok();
    Ok(axum::Json(adopt(&state.state_dir, &name, value)?))
}

async fn delete_secret(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let removed = remove(&state.state_dir, &name)?;
    Ok(axum::Json(serde_json::json!({ "removed": removed })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("agentdock-secrets-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn stored_secrets_resolve_and_are_private() {
        let dir = directory();
        store(&dir, "AGENTDOCK_SECRET_FILE_ONLY_A", "sk-one").unwrap();
        store(&dir, "AGENTDOCK_SECRET_FILE_ONLY_B", "sk-two").unwrap();
        assert_eq!(
            resolve(&dir, "AGENTDOCK_SECRET_FILE_ONLY_A").as_deref(),
            Some("sk-one")
        );
        assert_eq!(
            resolve_reference(&dir, "env:AGENTDOCK_SECRET_FILE_ONLY_B").unwrap(),
            "sk-two"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path(&dir)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(remove(&dir, "AGENTDOCK_SECRET_FILE_ONLY_A").unwrap());
        assert!(!remove(&dir, "AGENTDOCK_SECRET_FILE_ONLY_A").unwrap());
        assert!(resolve(&dir, "AGENTDOCK_SECRET_FILE_ONLY_A").is_none());
        let names = list(&dir);
        assert!(names.contains(&SecretName {
            name: "AGENTDOCK_SECRET_FILE_ONLY_B".into(),
            source: "agentdock",
            stored: true,
        }));
        assert!(!fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        }));
    }

    #[test]
    fn a_key_from_the_environment_is_kept_once_adopted() {
        let dir = directory();
        let name = "AGENTDOCK_SECRET_ADOPTED_ONLY";
        assert!(adopt(&dir, name, None).is_err(), "nothing to keep");
        assert!(adopt(&dir, name, Some(String::new())).is_err());
        let adopted = adopt(&dir, name, Some("sk-from-env".into())).unwrap();
        assert!(adopted.stored);
        assert_eq!(
            read(&dir).get(name).map(String::as_str),
            Some("sk-from-env")
        );
        assert!(
            list(&dir)
                .iter()
                .any(|entry| entry.name == name && entry.stored)
        );
    }

    #[test]
    fn names_and_values_are_checked() {
        let dir = directory();
        for bad in [
            "HOME",
            "AGENTDOCK_SECRET_",
            "AGENTDOCK_SECRET_lower",
            "AGENTDOCK_TOKEN",
        ] {
            assert!(store(&dir, bad, "x").is_err(), "{bad}");
        }
        assert!(store(&dir, "AGENTDOCK_SECRET_X", "").is_err());
        assert!(store(&dir, "AGENTDOCK_SECRET_X", "a\0b").is_err());
        assert!(store(&dir, "AGENTDOCK_SECRET_X", &"x".repeat(8193)).is_err());
        assert!(resolve_reference(&dir, "AGENTDOCK_SECRET_X").is_err());
        assert!(resolve_reference(&dir, "env:AGENTDOCK_SECRET_MISSING_HERE").is_err());
    }

    #[test]
    fn a_corrupt_file_resolves_nothing_and_is_never_overwritten() {
        let dir = directory();
        fs::write(path(&dir), "not = [toml").unwrap();
        assert!(resolve(&dir, "AGENTDOCK_SECRET_ANY").is_none());
        assert!(store(&dir, "AGENTDOCK_SECRET_AFTER", "v").is_err());
        assert!(remove(&dir, "AGENTDOCK_SECRET_AFTER").is_err());
        assert_eq!(fs::read_to_string(path(&dir)).unwrap(), "not = [toml");
    }
}
