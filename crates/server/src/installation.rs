//! Installation/state paths are independent of the selected project workspace.
use agentdock_persistence::Store;
use std::{
    env, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;

/// Keep this value alive until the server has shut down. The store drops before
/// its ownership locks; cloning the store does not clone or release the locks.
pub struct LockedDatabase {
    pub store: Arc<Store>,
    _ownership: OwnershipLocks,
}

pub struct PreparedServer {
    pub listener: TcpListener,
    pub database: LockedDatabase,
}

/// Reserve the listening socket before opening SQLite. A second launch on the
/// same port must not migrate the live database or reconcile its running rows.
pub async fn prepare_server(
    address: SocketAddr,
    state_dir: &Path,
    database: &Path,
) -> Result<PreparedServer, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(address).await.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot listen on {address}; check whether AgentDock is already running: {error}"
            ),
        )
    })?;
    Ok(PreparedServer {
        listener,
        database: open_database(state_dir, database)?,
    })
}

/// Both `init` and `serve` use the same ownership boundary before Store::open
/// can run migrations. This helper deliberately never reconciles sessions.
pub fn open_database(
    state_dir: &Path,
    database: &Path,
) -> Result<LockedDatabase, Box<dyn std::error::Error>> {
    let ownership = OwnershipLocks::acquire(state_dir, database)?;
    Ok(LockedDatabase {
        store: Arc::new(Store::open(database)?),
        _ownership: ownership,
    })
}

struct OwnershipLocks {
    _state: OwnershipLock,
    _database: Option<OwnershipLock>,
}

struct OwnershipLock {
    file: fs::File,
}

impl Drop for OwnershipLock {
    fn drop(&mut self) {
        // A concurrent fork can briefly inherit the same open-file description
        // until exec closes its CLOEXEC descriptor. Closing our handle alone
        // would leave flock held until that copy closes. Explicitly unlock on
        // normal release, including rollback after acquiring only the first lock.
        // A crash still relies on the OS closing the handles.
        let _ = self.file.unlock();
    }
}

impl OwnershipLocks {
    fn acquire(state_dir: &Path, database: &Path) -> io::Result<Self> {
        let state_dir = fs::canonicalize(state_dir)?;
        let state = lock_file(&state_dir.join("server.lock"), "state directory")?;
        // In-memory SQLite databases have no shared file identity, but the
        // state directory still has a single owner for processes and profiles.
        let database = if database == Path::new(":memory:") {
            None
        } else {
            let canonical = canonical_database(database)?;
            let mut lock_name = canonical
                .file_name()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid database path")
                })?
                .to_os_string();
            lock_name.push(".agentdock.lock");
            Some(lock_file(&canonical.with_file_name(lock_name), "database")?)
        };
        Ok(Self {
            _state: state,
            _database: database,
        })
    }
}

fn canonical_database(database: &Path) -> io::Result<PathBuf> {
    let absolute = if database.is_absolute() {
        database.to_path_buf()
    } else {
        env::current_dir()?.join(database)
    };
    match fs::canonicalize(&absolute) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Resolve the existing parent for a new DB as well, so alternate
            // paths/symlinked directories agree before either process creates it.
            if fs::symlink_metadata(&absolute).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Database symlink target is unavailable",
                ));
            }
            let parent = absolute.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Database parent is unavailable",
                )
            })?;
            let name = absolute.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid database filename")
            })?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

fn lock_file(path: &Path, resource: &str) -> io::Result<OwnershipLock> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AgentDock ownership lock must be a regular file, not a symbolic link",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "AgentDock {resource} is already in use by another server or initialization. Stop that instance or choose a separate AGENTDOCK_HOME and AGENTDOCK_DB."
            ),
        ),
        fs::TryLockError::Error(error) => io::Error::new(
            error.kind(),
            format!("Cannot acquire the AgentDock {resource} ownership lock: {error}"),
        ),
    })?;
    // Never unlink this file on release: a waiting/new owner may already hold
    // its inode, and unlinking would allow a different lock file to be created.
    Ok(OwnershipLock { file })
}

pub fn state_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let explicit = env::var_os("AGENTDOCK_STATE_DIR")
        .or_else(|| env::var_os("AGENTDOCK_HOME"))
        .map(PathBuf::from);
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    choose_state(explicit, home, &env::current_dir()?)
}
fn choose_state(
    explicit: Option<PathBuf>,
    home: Option<PathBuf>,
    cwd: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = explicit {
        return Ok(if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        });
    }
    let legacy = cwd.join(".agentdock");
    if legacy.join("agentdock.db").is_file() {
        return Ok(legacy);
    }
    Ok(home
        .ok_or("No user home available; set AGENTDOCK_HOME")?
        .join(".agentdock"))
}
pub fn web_directory(state_dir: &Path) -> PathBuf {
    asset_path(
        "AGENTDOCK_WEB_DIR",
        state_dir.join("web"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist"),
    )
}
pub fn native_bridge(state_dir: &Path) -> PathBuf {
    asset_path(
        "AGENTDOCK_NATIVE_BRIDGE",
        state_dir.join("native-bridge/history.mjs"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/native-bridge/history.mjs"),
    )
}
fn asset_path(key: &str, installed: PathBuf, development: PathBuf) -> PathBuf {
    if let Some(path) = env::var_os(key) {
        return PathBuf::from(path);
    }
    if installed.exists() {
        installed
    } else {
        development
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use agentdock_domain::{ProviderKind, SessionId, SessionStatus};

    struct TempInstallation {
        path: PathBuf,
        state: PathBuf,
        database: PathBuf,
    }
    impl TempInstallation {
        fn new() -> Self {
            let path = env::temp_dir().join(format!("agentdock-startup-{}", uuid::Uuid::new_v4()));
            let state = path.join("state");
            fs::create_dir_all(&state).unwrap();
            let database = path.join("sessions.db");
            Self {
                path,
                state,
                database,
            }
        }
        fn running_session(&self) -> SessionId {
            let store = Store::open(&self.database).unwrap();
            let workspace = store
                .create_workspace("fixture", self.path.to_str().unwrap())
                .unwrap();
            let session = store
                .create_session(workspace.id, ProviderKind::Terminal, "Live fixture")
                .unwrap();
            store
                .set_session_result(
                    session.id,
                    SessionStatus::Running,
                    Some("keep this diagnostic"),
                )
                .unwrap();
            session.id
        }
        fn assert_still_running(&self, id: SessionId) {
            let store = Store::open(&self.database).unwrap();
            let session = store.get_session(id).unwrap().unwrap();
            assert!(matches!(session.status, SessionStatus::Running));
            assert_eq!(session.error.as_deref(), Some("keep this diagnostic"));
        }
        fn second_state(&self) -> PathBuf {
            let state = self.path.join("other-state");
            fs::create_dir(&state).unwrap();
            state
        }
    }
    impl Drop for TempInstallation {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn assert_in_use(error: &(dyn std::error::Error + 'static)) {
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(error.to_string().contains("already in use"));
    }

    #[test]
    fn new_installs_use_home_and_explicit_paths_win() {
        assert_eq!(
            choose_state(
                None,
                Some("/home/tester".into()),
                Path::new("/unlikely/agentdock/cwd")
            )
            .unwrap(),
            PathBuf::from("/home/tester/.agentdock")
        );
        assert_eq!(
            choose_state(Some("/srv/ad".into()), None, Path::new("/other")).unwrap(),
            PathBuf::from("/srv/ad")
        );
    }
    #[test]
    fn existing_local_state_is_never_silently_moved() {
        let base = std::env::temp_dir().join(format!("agentdock-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join(".agentdock")).unwrap();
        std::fs::write(base.join(".agentdock/agentdock.db"), b"test").unwrap();
        assert_eq!(
            choose_state(None, Some("/home/tester".into()), &base).unwrap(),
            base.join(".agentdock")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn occupied_port_fails_before_opening_or_reconciling_database() {
        let fixture = TempInstallation::new();
        let session = fixture.running_session();
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = occupied.local_addr().unwrap();
        let failure = prepare_server(address, &fixture.state, &fixture.database)
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
        assert!(
            !fixture.state.join("server.lock").exists(),
            "Binding must happen before state ownership or SQLite is opened"
        );
        fixture.assert_still_running(session);
        // A failed launch must not create a previously missing database either.
        let missing = fixture.path.join("not-created.db");
        assert!(
            prepare_server(address, &fixture.state, &missing)
                .await
                .is_err()
        );
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn different_ports_and_state_directories_cannot_share_database_ownership() {
        let fixture = TempInstallation::new();
        let session = fixture.running_session();
        let first = prepare_server(
            "127.0.0.1:0".parse().unwrap(),
            &fixture.state,
            &fixture.database,
        )
        .await
        .unwrap();
        let other_state = fixture.second_state();
        // Every port 0 reservation chooses another free listener. It still
        // cannot acquire ownership of the same canonical database.
        let failure = prepare_server(
            "127.0.0.1:0".parse().unwrap(),
            &other_state,
            &fixture.database,
        )
        .await
        .err()
        .unwrap();
        assert_in_use(failure.as_ref());
        fixture.assert_still_running(session);
        assert!(first.listener.local_addr().unwrap().port() > 0);
        // The partially acquired second state's lock is released on failure.
        let separate = open_database(&other_state, &fixture.path.join("separate.db")).unwrap();
        assert!(separate.store.list_sessions(None).unwrap().is_empty());
        drop(separate);
        drop(first);
        // Lock marker files persist, but ownership is released with the handle.
        let reopened = open_database(&fixture.state, &fixture.database).unwrap();
        assert!(matches!(
            reopened.store.get_session(session).unwrap().unwrap().status,
            SessionStatus::Running
        ));
    }

    #[test]
    fn same_state_cannot_be_shared_even_with_different_databases() {
        let fixture = TempInstallation::new();
        let first = open_database(&fixture.state, &fixture.database).unwrap();
        let other = fixture.path.join("other.db");
        let failure = open_database(&fixture.state, &other).err().unwrap();
        assert_in_use(failure.as_ref());
        assert!(!other.exists());
        drop(first);
        assert!(open_database(&fixture.state, &other).is_ok());
    }

    #[test]
    fn initialization_is_exclusive_but_never_reconciles_existing_sessions() {
        let fixture = TempInstallation::new();
        let session = fixture.running_session();
        let initialized = open_database(&fixture.state, &fixture.database).unwrap();
        let existing = initialized.store.get_session(session).unwrap().unwrap();
        assert!(matches!(existing.status, SessionStatus::Running));
        assert_eq!(existing.error.as_deref(), Some("keep this diagnostic"));
        let other_state = fixture.second_state();
        let denied = open_database(&other_state, &fixture.database)
            .err()
            .unwrap();
        assert_in_use(denied.as_ref());
        drop(initialized);
        fixture.assert_still_running(session);
        let initialized_again = open_database(&fixture.state, &fixture.database).unwrap();
        let after = initialized_again
            .store
            .get_session(session)
            .unwrap()
            .unwrap();
        assert_eq!(after.updated_at, existing.updated_at);
        assert!(matches!(after.status, SessionStatus::Running));
    }

    #[cfg(unix)]
    #[test]
    fn database_symlink_and_parent_aliases_share_the_same_lock_identity() {
        use std::os::unix::fs::symlink;
        let fixture = TempInstallation::new();
        fixture.running_session();
        let first = open_database(&fixture.state, &fixture.database).unwrap();
        let other_state = fixture.second_state();
        let alias = fixture.path.join("alias.db");
        symlink(&fixture.database, &alias).unwrap();
        let failure = open_database(&other_state, &alias).err().unwrap();
        assert_in_use(failure.as_ref());
        let directory_alias = fixture.path.join("parent-alias");
        symlink(&fixture.path, &directory_alias).unwrap();
        let failure = open_database(&other_state, &directory_alias.join("sessions.db"))
            .err()
            .unwrap();
        assert_in_use(failure.as_ref());
        drop(first);
        let _reopened = open_database(&other_state, &alias).unwrap();
    }

    #[test]
    fn failed_database_open_releases_both_ownership_locks() {
        let fixture = TempInstallation::new();
        fs::write(&fixture.database, b"not a sqlite database").unwrap();
        assert!(open_database(&fixture.state, &fixture.database).is_err());
        // Retain the invalid fixture for diagnosis; use another DB to prove
        // state ownership was not leaked by the failed migration/open.
        let valid = open_database(&fixture.state, &fixture.path.join("valid.db")).unwrap();
        assert!(valid.store.list_sessions(None).unwrap().is_empty());
        let other_state = fixture.second_state();
        let locks = OwnershipLocks::acquire(&other_state, &fixture.database).unwrap();
        drop(locks);
    }

    #[test]
    fn ownership_release_unlocks_even_if_a_descriptor_copy_is_still_open() {
        let fixture = TempInstallation::new();
        let path = fixture.state.join("descriptor-copy.lock");
        let first = lock_file(&path, "test fixture").unwrap();
        // try_clone deterministically models the open-file-description sharing
        // that can occur during another thread's fork-before-exec window.
        let inherited_descriptor = first.file.try_clone().unwrap();
        assert!(
            matches!(lock_file(&path, "test fixture"), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        drop(first);
        let second = lock_file(&path, "test fixture").unwrap();
        drop(inherited_descriptor);
        // Closing the previous owner's copy must not release the new owner.
        assert!(
            matches!(lock_file(&path, "test fixture"), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        drop(second);
        assert!(lock_file(&path, "test fixture").is_ok());
    }
}
