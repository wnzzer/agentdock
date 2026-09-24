//! Safe workspace file and Git operations.
//!
//! The module deliberately has no HTTP/router dependencies.  Callers provide a
//! workspace root and receive serialisable domain values or an [`IoError`]
//! carrying a useful HTTP status code.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;

const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_GIT_OUTPUT: usize = 8 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const COMMIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Structured error returned by workspace operations.
#[derive(Debug, Clone, Serialize)]
pub struct IoError {
    pub status: u16,
    pub message: String,
}

impl IoError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn io(error: io::Error) -> Self {
        let status = match error.kind() {
            io::ErrorKind::NotFound => 404,
            io::ErrorKind::PermissionDenied => 403,
            io::ErrorKind::AlreadyExists => 409,
            _ => 500,
        };
        Self::new(status, error.to_string())
    }
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IoError {}

impl From<io::Error> for IoError {
    fn from(value: io::Error) -> Self {
        Self::io(value)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextFile {
    pub path: String,
    pub content: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitFile {
    pub index: String,
    pub worktree: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub files: Vec<GitFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitDiff {
    pub diff: String,
    pub path: Option<String>,
    pub staged: bool,
    pub binary: bool,
    pub truncated: bool,
}

/// Resolve a relative workspace path while preventing traversal and symlink
/// escapes. Existing paths are canonicalised; for a new path the canonical
/// parent is checked and the original (non-existent) path is returned.
pub fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, IoError> {
    let canonical_root = fs::canonicalize(root).map_err(IoError::from)?;
    if !canonical_root.is_dir() {
        return Err(IoError::new(400, "workspace root is not a directory"));
    }
    let rel = safe_relative(relative)?;
    let candidate = canonical_root.join(&rel);
    match fs::symlink_metadata(&candidate) {
        Ok(_) => {
            let canonical = fs::canonicalize(&candidate).map_err(IoError::from)?;
            if !canonical.starts_with(&canonical_root) {
                return Err(IoError::new(403, "path escapes workspace root"));
            }
            Ok(canonical)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // For a new file (or a new nested directory), walk up to the
            // nearest existing ancestor and canonicalise that component. This
            // still catches a symlinked ancestor while allowing callers to
            // create `src/new/file.txt` in one operation.
            let mut parent = candidate
                .parent()
                .ok_or_else(|| IoError::new(400, "invalid path"))?;
            while !parent.exists() {
                parent = parent
                    .parent()
                    .ok_or_else(|| IoError::new(400, "invalid path"))?;
            }
            let canonical_parent = fs::canonicalize(parent).map_err(IoError::from)?;
            if !canonical_parent.starts_with(&canonical_root)
                || is_protected_path(
                    canonical_parent
                        .strip_prefix(&canonical_root)
                        .unwrap_or(&canonical_parent),
                )
            {
                return Err(IoError::new(403, "path escapes workspace root"));
            }
            Ok(candidate)
        }
        Err(error) => Err(IoError::from(error)),
    }
}

fn safe_relative(relative: &str) -> Result<PathBuf, IoError> {
    if relative.as_bytes().contains(&0) {
        return Err(IoError::new(400, "path contains NUL"));
    }
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(IoError::new(400, "absolute paths are not allowed"));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => result.push(part),
            Component::ParentDir => return Err(IoError::new(400, "path traversal is not allowed")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(IoError::new(400, "absolute paths are not allowed"));
            }
        }
    }
    Ok(result)
}

fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_protected_path(relative: &Path) -> bool {
    // AgentDock must never mutate repository metadata, credentials, or
    // provider configuration.  These names are intentionally conservative.
    const PROTECTED: &[&str] = &[
        ".git",
        ".agentdock",
        ".claude.json",
        ".ssh",
        ".aws",
        ".azure",
        ".config",
        ".claude",
        ".codex",
        "credentials",
        "credential",
        "secrets",
    ];
    relative.components().any(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        let value = part.to_string_lossy();
        PROTECTED.iter().any(|name| value == *name)
    })
}

/// What the tree calls an entry, from the metadata of the entry itself. A link
/// is a link here rather than what it resolves to, which is what a rename acts
/// on and what the tree already shows.
fn entry_kind(metadata: &fs::Metadata) -> String {
    if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
    .into()
}

fn has_symlink_component(root: &Path, relative: &Path) -> Result<bool, IoError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(IoError::from(error)),
        }
    }
    Ok(false)
}

fn hash_version(bytes: &[u8]) -> String {
    // Stable, dependency-free content hash. Prefix permits a future migration
    // to SHA-256 without invalidating the optimistic-concurrency contract.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("v1:{hash:016x}")
}

pub async fn list_files(root: &Path, relative: &str) -> Result<Vec<FileEntry>, IoError> {
    let root = root.to_path_buf();
    let relative = relative.to_owned();
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let directory = safe_path(&canonical_root, &relative)?;
        let metadata = fs::metadata(&directory).map_err(IoError::from)?;
        if !metadata.is_dir() {
            return Err(IoError::new(400, "path is not a directory"));
        }
        let mut entries = Vec::new();
        if is_protected_path(
            directory
                .strip_prefix(&canonical_root)
                .unwrap_or(&directory),
        ) {
            return Err(IoError::new(403, "protected directory"));
        }
        for (index, item) in fs::read_dir(&directory).map_err(IoError::from)?.enumerate() {
            if index >= 10000 {
                return Err(IoError::new(413, "Directory exceeds 10,000 entries"));
            }

            let item = item.map_err(IoError::from)?;
            let path = item.path();
            let lexical_metadata = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            let Ok(safe) = safe_path(&canonical_root, &relative_string(&canonical_root, &path))
            else {
                continue; // Hide symlinks that leave the workspace.
            };
            let is_symlink = lexical_metadata.file_type().is_symlink();
            let metadata = match fs::metadata(&safe) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            // Preserve the user's lexical symlink path in listings while still
            // validating its target via `safe_path` above.
            let rel = relative_string(&canonical_root, &path);
            if is_protected_path(Path::new(&rel))
                || is_protected_path(safe.strip_prefix(&canonical_root).unwrap_or(&safe))
            {
                continue;
            }
            entries.push(FileEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                path: rel,
                kind: if is_symlink {
                    "symlink"
                } else if metadata.is_dir() {
                    "directory"
                } else if metadata.is_file() {
                    "file"
                } else {
                    "other"
                }
                .into(),
                size: metadata.len(),
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != "directory")
                .cmp(&(b.kind != "directory"))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    })
    .await
    .map_err(|error| IoError::new(500, format!("file listing task failed: {error}")))?
}

pub async fn read_file(root: &Path, path: &str) -> Result<TextFile, IoError> {
    let root = root.to_path_buf();
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let rel = safe_relative(&path)?;
        let target = asset_path(&canonical_root, &path)?;
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path"));
        }
        let metadata = fs::metadata(&target).map_err(IoError::from)?;
        if !metadata.is_file() {
            return Err(IoError::new(400, "path is not a file"));
        }
        if metadata.len() > MAX_TEXT_BYTES {
            return Err(IoError::new(413, "text file exceeds 2 MiB limit"));
        }
        let bytes = fs::read(&target).map_err(IoError::from)?;
        if bytes.len() as u64 > MAX_TEXT_BYTES {
            return Err(IoError::new(413, "text file exceeds 2 MiB limit"));
        }
        let content = String::from_utf8(bytes.clone())
            .map_err(|_| IoError::new(415, "file is not valid UTF-8"))?;
        Ok(TextFile {
            path: rel.to_string_lossy().replace('\\', "/"),
            content,
            version: hash_version(&bytes),
        })
    })
    .await
    .map_err(|error| IoError::new(500, format!("file read task failed: {error}")))?
}

pub async fn write_file(
    root: &Path,
    path: &str,
    content: &str,
    expected_version: Option<&str>,
) -> Result<TextFile, IoError> {
    if content.len() as u64 > MAX_TEXT_BYTES {
        return Err(IoError::new(413, "text file exceeds 2 MiB limit"));
    }
    let root = root.to_path_buf();
    let path = path.to_owned();
    let content = content.to_owned();
    let expected_version = expected_version.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let rel = safe_relative(&path)?;
        if rel.as_os_str().is_empty() {
            return Err(IoError::new(400, "file path is required"));
        }
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path cannot be written"));
        }
        if has_symlink_component(&canonical_root, &rel)? {
            return Err(IoError::new(403, "symlink writes are not allowed"));
        }
        // Inspect the lexical target before canonicalisation so a symlink to a
        // location inside the workspace cannot be silently overwritten.
        let lexical_target = canonical_root.join(&rel);
        if let Ok(meta) = fs::symlink_metadata(&lexical_target)
            && meta.file_type().is_symlink()
        {
            return Err(IoError::new(403, "symlink writes are not allowed"));
        }
        let target = safe_path(&canonical_root, &path)?;
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if meta.file_type().is_symlink() {
                return Err(IoError::new(403, "symlink writes are not allowed"));
            }
            if !meta.is_file() {
                return Err(IoError::new(400, "path is not a regular file"));
            }
            if meta.len() > MAX_TEXT_BYTES {
                return Err(IoError::new(413, "Text exceeds 2 MiB limit"));
            }
            if expected_version.is_none() {
                return Err(IoError::new(
                    409,
                    "Read the current version before overwriting an existing file",
                ));
            }
            let existing = fs::read(&target).map_err(IoError::from)?;
            let actual = hash_version(&existing);
            if let Some(expected) = expected_version.as_deref()
                && expected != actual
            {
                return Err(IoError::new(409, "file changed since it was read"));
            }
        } else if expected_version.is_some() {
            return Err(IoError::new(409, "file no longer exists"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| IoError::new(400, "invalid file path"))?;
        fs::create_dir_all(parent).map_err(IoError::from)?;
        let temp = parent.join(format!(".agentdock-{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&temp).map_err(IoError::from)?;
        use io::Write;
        if let Err(error) = file
            .write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = fs::remove_file(&temp);
            return Err(IoError::from(error));
        }
        if let Ok(meta) = fs::metadata(&target) {
            let _ = fs::set_permissions(&temp, meta.permissions());
        }
        if let Err(error) = fs::rename(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(IoError::from(error));
        }
        let bytes = content.into_bytes();
        Ok(TextFile {
            path: rel.to_string_lossy().replace('\\', "/"),
            content: String::from_utf8(bytes.clone()).expect("input is UTF-8"),
            version: hash_version(&bytes),
        })
    })
    .await
    .map_err(|error| IoError::new(500, format!("file write task failed: {error}")))?
}

fn git_command(root: &Path, args: &[String]) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("GIT_")) {
        command.env_remove(key);
    }
    command
        .args([
            "--literal-pathspecs",
            "-c",
            "color.ui=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .current_dir(root)
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}
async fn run_git(
    root: &Path,
    args: Vec<String>,
    duration: Duration,
) -> Result<(Vec<u8>, Vec<u8>, bool), IoError> {
    let mut child = git_command(root, &args).spawn()?;
    let out = child
        .stdout
        .take()
        .ok_or_else(|| IoError::new(500, "missing Git stdout"))?;
    let err = child
        .stderr
        .take()
        .ok_or_else(|| IoError::new(500, "missing Git stderr"))?;
    async fn read_capped(stream: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, IoError> {
        let mut data = Vec::new();
        stream
            .take((MAX_GIT_OUTPUT + 1) as u64)
            .read_to_end(&mut data)
            .await?;
        if data.len() > MAX_GIT_OUTPUT {
            return Err(IoError::new(413, "Git output exceeds 8 MiB limit"));
        }
        Ok(data)
    }
    let result = timeout(duration, async {
        let (stdout, stderr) = tokio::try_join!(read_capped(out), read_capped(err))?;
        let status = child.wait().await?;
        Ok::<_, IoError>((stdout, stderr, status.success()))
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(IoError::new(504, "Git command timed out"))
        }
    }
}

pub async fn git_status(root: &Path) -> Result<GitStatus, IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    let (stdout, stderr, success) = run_git(
        root.as_path(),
        vec![
            "status".into(),
            "--porcelain=v1".into(),
            "-z".into(),
            "--branch".into(),
            "--untracked-files=all".into(),
        ],
        GIT_TIMEOUT,
    )
    .await?;
    if !success {
        return Err(IoError::new(400, git_error(&stderr)));
    }
    Ok(parse_git_status(&stdout))
}

fn parse_git_status(bytes: &[u8]) -> GitStatus {
    let mut branch = None;
    let mut ahead = None;
    let mut behind = None;
    let mut files = Vec::new();
    let mut tokens = bytes.split(|b| *b == 0).peekable();
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        if token.starts_with(b"## ") {
            let value = String::from_utf8_lossy(&token[3..]).to_string();
            let (name, tracking) = value
                .split_once("...")
                .map_or((value.as_str(), ""), |(n, t)| (n, t));
            branch = Some(
                if let Some(branch_name) = value.strip_prefix("No commits yet on ") {
                    branch_name.to_owned()
                } else {
                    name.split(' ').next().unwrap_or(name).to_string()
                },
            );
            if let Some(start) = tracking.find("[ahead ")
                && let Some(n) = tracking[start + 7..]
                    .split([',', ']'])
                    .next()
                    .and_then(|n| n.parse().ok())
            {
                ahead = Some(n);
            }
            if let Some(start) = tracking.find("behind ")
                && let Some(n) = tracking[start + 7..]
                    .split([',', ']'])
                    .next()
                    .and_then(|n| n.parse().ok())
            {
                behind = Some(n);
            }
            continue;
        }
        if token.len() < 3 {
            continue;
        }
        let index = String::from_utf8_lossy(&token[0..1]).to_string();
        let worktree = String::from_utf8_lossy(&token[1..2]).to_string();
        let path = String::from_utf8_lossy(&token[3..]).to_string();
        let original_path = if index == "R" || index == "C" || worktree == "R" || worktree == "C" {
            tokens
                .next()
                .map(|raw| String::from_utf8_lossy(raw).to_string())
        } else {
            None
        };
        files.push(GitFile {
            index,
            worktree,
            path,
            original_path,
        });
    }
    GitStatus {
        branch,
        files,
        ahead,
        behind,
    }
}

fn git_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() {
        "git command failed".into()
    } else {
        text
    }
}

pub async fn git_diff(root: &Path, path: Option<&str>, staged: bool) -> Result<GitDiff, IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    let safe = if let Some(path) = path {
        let rel = safe_relative(path)?;
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path"));
        }
        let _ = safe_path(&root, path)?;
        Some(path.to_owned())
    } else {
        None
    };
    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--cached".into());
    }
    args.extend([
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        "--no-color".into(),
        "--binary".into(),
    ]);
    if let Some(ref path) = safe {
        args.extend(["--".into(), path.clone()]);
    }
    let (stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if !success {
        return Err(IoError::new(400, git_error(&stderr)));
    }
    let mut diff = String::from_utf8_lossy(&stdout).to_string();
    // Ask Git for individual paths: obey ignore rules, never walk symlinked folders.
    if !staged {
        let (raw, stderr, ok) = run_git(
            &root,
            vec![
                "ls-files".into(),
                "--others".into(),
                "--exclude-standard".into(),
                "-z".into(),
            ],
            GIT_TIMEOUT,
        )
        .await?;
        if !ok {
            return Err(IoError::new(400, git_error(&stderr)));
        }
        let candidates: Vec<_> = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
        if candidates.len() > 500 {
            return Err(IoError::new(
                413,
                "Select a file to review this large untracked set",
            ));
        }
        for raw in candidates {
            let untracked = String::from_utf8_lossy(raw).into_owned();
            if safe.as_ref().is_some_and(|p| p != &untracked) {
                continue;
            }
            if is_protected_path(Path::new(&untracked)) {
                continue;
            }
            let target = match asset_path(&root, &untracked) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !target.is_file() {
                continue;
            }
            let (out, _, _) = run_git(
                &root,
                vec![
                    "diff".into(),
                    "--no-index".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-color".into(),
                    "--".into(),
                    "/dev/null".into(),
                    untracked,
                ],
                GIT_TIMEOUT,
            )
            .await?;
            if diff.len() + out.len() > MAX_GIT_OUTPUT {
                return Err(IoError::new(413, "Diff exceeds limit; select one file"));
            }
            diff.push_str(&String::from_utf8_lossy(&out));
        }
    }

    let binary = diff.contains("Binary files") || diff.contains("GIT binary patch");
    Ok(GitDiff {
        diff,
        path: safe,
        staged,
        binary,
        truncated: false,
    })
}

/// Shared guard for media and text reads. Both lexical and resolved paths matter.
pub fn asset_path(root: &Path, relative: &str) -> Result<PathBuf, IoError> {
    let canonical = fs::canonicalize(root)?;
    let rel = safe_relative(relative)?;
    if is_protected_path(&rel) {
        return Err(IoError::new(403, "protected path"));
    }
    let target = safe_path(&canonical, relative)?;
    if is_protected_path(
        target
            .strip_prefix(&canonical)
            .map_err(|_| IoError::new(403, "path escapes workspace"))?,
    ) {
        return Err(IoError::new(403, "protected path"));
    }
    if !target.is_file() {
        return Err(IoError::new(404, "file not found"));
    }
    Ok(target)
}

pub async fn git_stage(root: &Path, paths: &[String]) -> Result<(), IoError> {
    git_index_op(root, "add", paths).await
}

pub async fn git_unstage(root: &Path, paths: &[String]) -> Result<(), IoError> {
    git_index_op(root, "reset", paths).await
}

async fn git_index_op(root: &Path, operation: &str, paths: &[String]) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = if operation == "reset"
        && !run_git(
            &root,
            vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
            GIT_TIMEOUT,
        )
        .await?
        .2
    {
        vec![
            "rm".into(),
            "--cached".into(),
            "--ignore-unmatch".into(),
            "--".into(),
        ]
    } else {
        vec![operation.to_owned(), "--".into()]
    };
    if paths.len() > 1000 {
        return Err(IoError::new(413, "Too many paths"));
    }
    for path in paths {
        if path.is_empty() || path == "." {
            return Err(IoError::new(400, "Explicit file paths required"));
        }
        let rel = safe_relative(path)?;
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path"));
        }
        let _ = safe_path(&root, path)?;
        args.push(path.clone());
    }
    let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if success {
        Ok(())
    } else {
        Err(IoError::new(400, git_error(&stderr)))
    }
}

/// Throw away working-tree changes to the named files.
///
/// A tracked file goes back to what is staged for it -- the index, not HEAD --
/// so discarding never touches work that was already staged. An untracked file
/// has nothing to go back to and is removed, through the same guarded delete
/// the file explorer uses, which never follows a link out of the workspace.
/// Paths are named one by one: there is no "discard everything" spelling.
pub async fn git_discard(root: &Path, paths: &[String]) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    if paths.is_empty() {
        return Ok(());
    }
    if paths.len() > 1000 {
        return Err(IoError::new(413, "Too many paths"));
    }
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for path in paths {
        if path.is_empty() || path == "." {
            return Err(IoError::new(400, "Explicit file paths required"));
        }
        let rel = safe_relative(path)?;
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path"));
        }
        let known = run_git(
            &root,
            vec![
                "ls-files".into(),
                "--error-unmatch".into(),
                "--".into(),
                path.clone(),
            ],
            GIT_TIMEOUT,
        )
        .await?
        .2;
        if known {
            tracked.push(path.clone());
        } else {
            untracked.push(path.clone());
        }
    }
    if !tracked.is_empty() {
        let mut args = vec!["checkout".into(), "--".into()];
        args.extend(tracked);
        let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
        if !success {
            return Err(IoError::new(400, git_error(&stderr)));
        }
    }
    for path in untracked {
        delete_entry(&root, &path).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    /// The repository's own checkout, as opposed to one added beside it.
    pub main: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitBranches {
    /// None when HEAD is detached.
    pub current: Option<String>,
    pub branches: Vec<String>,
    pub worktrees: Vec<GitWorktree>,
}

async fn git_text(root: &Path, args: &[&str]) -> Result<Option<String>, IoError> {
    let (stdout, _stderr, success) = run_git(
        root,
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        GIT_TIMEOUT,
    )
    .await?;
    Ok(success.then(|| String::from_utf8_lossy(&stdout).trim().to_owned()))
}

/// Local branches and the worktrees of this repository.
pub async fn git_branches(root: &Path) -> Result<GitBranches, IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    let branches = git_text(&root, &["branch", "--format=%(refname:short)"])
        .await?
        .ok_or_else(|| IoError::new(400, "Not a Git repository"))?;
    let current = git_text(&root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .await?
        .filter(|name| !name.is_empty());
    let listing = git_text(&root, &["worktree", "list", "--porcelain"])
        .await?
        .unwrap_or_default();
    Ok(GitBranches {
        current,
        branches: branches
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.starts_with('('))
            .map(str::to_owned)
            .collect(),
        worktrees: parse_worktrees(&listing),
    })
}

fn parse_worktrees(listing: &str) -> Vec<GitWorktree> {
    listing
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            let mut bare = false;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(value.to_owned());
                } else if let Some(value) = line.strip_prefix("branch ") {
                    branch = Some(value.trim_start_matches("refs/heads/").to_owned());
                } else if line == "bare" {
                    bare = true;
                }
            }
            (!bare).then_some(())?;
            path.map(|path| GitWorktree {
                path,
                branch,
                main: false,
            })
        })
        .enumerate()
        .map(|(index, mut worktree)| {
            worktree.main = index == 0;
            worktree
        })
        .collect()
}

/// A name Git itself accepts for a branch, checked by Git rather than guessed.
async fn valid_branch(root: &Path, branch: &str) -> Result<(), IoError> {
    if branch.is_empty() || branch.len() > 200 || branch.starts_with('-') {
        return Err(IoError::new(400, "Invalid branch name"));
    }
    match git_text(root, &["check-ref-format", "--branch", branch]).await? {
        Some(_) => Ok(()),
        None => Err(IoError::new(400, "Invalid branch name")),
    }
}

/// Switch this checkout to another local branch, or create one here.
/// Git refuses a switch that would overwrite uncommitted work, and that
/// refusal is passed on rather than forced through.
pub async fn git_switch(root: &Path, branch: &str, create: bool) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    valid_branch(&root, branch).await?;
    let mut args: Vec<String> = vec!["switch".into()];
    if create {
        args.push("-c".into());
    }
    args.push(branch.to_owned());
    let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if success {
        Ok(())
    } else {
        Err(IoError::new(409, git_error(&stderr)))
    }
}

/// Rename a local branch. A worktree that has it checked out follows the new
/// name -- Git updates it -- so no checkout has to move.
pub async fn git_branch_rename(root: &Path, from: &str, to: &str) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    valid_branch(&root, from).await?;
    valid_branch(&root, to).await?;
    let args = vec!["branch".into(), "-m".into(), from.to_owned(), to.to_owned()];
    let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if success {
        Ok(())
    } else {
        Err(IoError::new(409, git_error(&stderr)))
    }
}

/// Delete a local branch. Git refuses one that is checked out anywhere, and
/// -- unless `force` -- one whose commits are not merged; both are passed on.
pub async fn git_branch_delete(root: &Path, branch: &str, force: bool) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    valid_branch(&root, branch).await?;
    let args = vec![
        "branch".into(),
        if force { "-D" } else { "-d" }.into(),
        branch.to_owned(),
    ];
    let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if success {
        Ok(())
    } else {
        Err(IoError::new(409, git_error(&stderr)))
    }
}

/// Remove a worktree of this repository -- its directory, not its branch.
/// Git refuses one with uncommitted changes unless `force`; the repository's
/// own checkout can never be removed this way.
pub async fn git_worktree_remove(root: &Path, path: &str, force: bool) -> Result<(), IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    let listing = git_text(&root, &["worktree", "list", "--porcelain"])
        .await?
        .ok_or_else(|| IoError::new(400, "Not a Git repository"))?;
    let tree = parse_worktrees(&listing)
        .into_iter()
        .find(|tree| tree.path == path)
        .ok_or_else(|| IoError::new(400, "Not a worktree of this repository"))?;
    if tree.main {
        return Err(IoError::new(
            400,
            "The repository's own checkout cannot be removed",
        ));
    }
    let mut args: Vec<String> = vec!["worktree".into(), "remove".into()];
    if force {
        args.push("--force".into());
    }
    args.push(tree.path);
    let (_stdout, stderr, success) = run_git(&root, args, GIT_TIMEOUT).await?;
    if success {
        Ok(())
    } else {
        Err(IoError::new(409, git_error(&stderr)))
    }
}

/// Where a new worktree for `branch` goes: beside the repository's own
/// checkout, in `<repo>.worktrees/<branch>`, so every worktree of one
/// repository sits together and none of them inside another.
pub fn worktree_path(main_checkout: &Path, branch: &str) -> Result<PathBuf, IoError> {
    let name = main_checkout
        .file_name()
        .ok_or_else(|| IoError::new(400, "The repository has no directory name"))?;
    let parent = main_checkout
        .parent()
        .ok_or_else(|| IoError::new(400, "The repository has no parent directory"))?;
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(|c| c == '-' || c == '.');
    if slug.is_empty() {
        return Err(IoError::new(400, "Invalid branch name"));
    }
    Ok(parent
        .join(format!("{}.worktrees", name.to_string_lossy()))
        .join(slug))
}

/// Add a worktree for `branch` (created from HEAD when `create`), returning
/// its directory. An existing directory is never reused or overwritten.
pub async fn git_worktree_add(root: &Path, branch: &str, create: bool) -> Result<PathBuf, IoError> {
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    valid_branch(&root, branch).await?;
    let listing = git_text(&root, &["worktree", "list", "--porcelain"])
        .await?
        .ok_or_else(|| IoError::new(400, "Not a Git repository"))?;
    let main = parse_worktrees(&listing)
        .into_iter()
        .find(|worktree| worktree.main)
        .ok_or_else(|| IoError::new(400, "Not a Git repository"))?;
    let target = worktree_path(Path::new(&main.path), branch)?;
    if target.exists() {
        return Err(IoError::new(
            409,
            format!("{} already exists", target.display()),
        ));
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
    if create {
        args.extend([
            "-b".into(),
            branch.to_owned(),
            target.to_string_lossy().into_owned(),
        ]);
    } else {
        args.extend([target.to_string_lossy().into_owned(), branch.to_owned()]);
    }
    let (_stdout, stderr, success) = run_git(&root, args, COMMIT_TIMEOUT).await?;
    if !success {
        return Err(IoError::new(409, git_error(&stderr)));
    }
    Ok(target)
}

pub async fn git_commit(root: &Path, message: &str) -> Result<String, IoError> {
    if message.trim().is_empty() {
        return Err(IoError::new(400, "commit message is required"));
    }
    let root = fs::canonicalize(root).map_err(IoError::from)?;
    let args = vec!["commit".into(), "-m".into(), message.to_owned()];
    let (_stdout, stderr, success) = run_git(&root, args, COMMIT_TIMEOUT).await?;
    if !success {
        return Err(IoError::new(400, git_error(&stderr)));
    }
    let (stdout, stderr, success) =
        run_git(&root, vec!["rev-parse".into(), "HEAD".into()], GIT_TIMEOUT).await?;
    if !success {
        return Err(IoError::new(400, git_error(&stderr)));
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
}

/// Directory attachments land in, relative to the workspace root. Deliberately
/// not `.agentdock`: that name is reserved for AgentDock's own server state, and
/// a workspace may be the AgentDock checkout itself.
pub const ATTACHMENT_DIR: &str = ".agentdock-files";
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    /// Workspace-relative path, which is what a session is given.
    pub path: String,
    pub name: String,
    pub bytes: u64,
}

/// Reduce a client-supplied filename to a single safe path component.
///
/// Only the base name is kept, so a path from a phone's file picker cannot
/// steer the write. The original stem and extension are preserved where they
/// are usable, because the agent reading this file benefits from a recognisable
/// name and a correct extension.
fn attachment_name(raw: &str) -> (String, String, String) {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '\0') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    let (stem, extension) = match cleaned.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty()
                && (1..=16).contains(&extension.len())
                && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            (
                stem.to_owned(),
                format!(".{}", extension.to_ascii_lowercase()),
            )
        }
        _ => (cleaned.clone(), String::new()),
    };
    let stem: String = stem.chars().take(80).collect();
    let stem = stem.trim().to_owned();
    let display = if stem.is_empty() {
        format!("attachment{extension}")
    } else {
        format!("{stem}{extension}")
    };
    // The stored path is handed to an agent as text, so whitespace collapses to
    // hyphens; a path it has to quote correctly is a path it can get wrong. The
    // display name keeps the user's original spelling.
    let mut slug = String::new();
    let mut pending_separator = false;
    for character in stem.chars() {
        if character.is_whitespace() || character == '-' {
            pending_separator = !slug.is_empty();
        } else {
            if pending_separator {
                slug.push('-');
                pending_separator = false;
            }
            slug.push(character);
        }
    }
    (display, extension, slug)
}

/// Store an uploaded attachment inside the workspace and return its relative
/// path. Nothing is overwritten: each upload gets its own unique file name, so
/// two photos with the same camera name both survive.
pub async fn write_attachment(
    root: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<Attachment, IoError> {
    if bytes.is_empty() {
        return Err(IoError::new(400, "attachment is empty"));
    }
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(IoError::new(413, "attachment exceeds 10 MiB limit"));
    }
    let (display, extension, stem) = attachment_name(name);
    let unique = Uuid::new_v4().simple().to_string();
    let relative = format!(
        "{ATTACHMENT_DIR}/{}/{}-{}{extension}",
        chrono::Utc::now().format("%Y-%m-%d"),
        if stem.is_empty() { "attachment" } else { &stem },
        &unique[..8]
    );
    let root = root.to_path_buf();
    let payload = bytes.to_vec();
    let size = payload.len() as u64;
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let rel = safe_relative(&relative)?;
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "protected path cannot be written"));
        }
        if has_symlink_component(&canonical_root, &rel)? {
            return Err(IoError::new(403, "symlink writes are not allowed"));
        }
        let target = safe_path(&canonical_root, &relative)?;
        if fs::symlink_metadata(&target).is_ok() {
            return Err(IoError::new(409, "attachment already exists"));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(IoError::from)?;
        }
        // A self-contained ignore, so uploads do not turn into pending Git
        // changes in the user's repository. It only covers this directory and
        // never edits a .gitignore the user owns.
        let ignore = canonical_root.join(ATTACHMENT_DIR).join(".gitignore");
        if fs::symlink_metadata(&ignore).is_err() {
            let _ = fs::write(&ignore, "*\n");
        }
        fs::write(&target, &payload).map_err(IoError::from)?;
        Ok(Attachment {
            path: relative_string(&canonical_root, &target),
            name: display,
            bytes: size,
        })
    })
    .await
    .map_err(|_| IoError::new(500, "attachment write failed"))?
}

/// Delete one file or directory inside a workspace.
///
/// `safe_path` already refuses anything that resolves outside the root or into
/// a protected name. Two things it cannot decide for a delete are handled here:
/// the root itself is never removable, and a symlink is unlinked rather than
/// followed — otherwise deleting a link inside the workspace would delete
/// whatever it points at.
/// Create an empty file the workspace does not have yet.
///
/// Writing would serve for a file that is new, but not for one that is not: an
/// overwrite is what a blank new file looks like to `write_file`, and losing
/// what was there is the one outcome this must never have. Refusing an existing
/// name says so instead.
///
/// Missing parent directories are created, so naming `docs/notes.md` in a
/// workspace without a `docs` makes both, the way typing the path suggests.
pub async fn create_file(root: &Path, relative: &str) -> Result<FileEntry, IoError> {
    let root = root.to_path_buf();
    let relative = relative.to_owned();
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let rel = safe_relative(&relative)?;
        if rel.as_os_str().is_empty() {
            return Err(IoError::new(400, "file name is required"));
        }
        if is_protected_path(&rel) {
            return Err(IoError::new(403, "this path is protected"));
        }
        if has_symlink_component(&canonical_root, &rel)? {
            return Err(IoError::new(403, "symlink writes are not allowed"));
        }
        let target = canonical_root.join(&rel);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(IoError::new(409, "something with this name already exists"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| IoError::new(400, "invalid file path"))?;
        fs::create_dir_all(parent).map_err(IoError::from)?;
        // `create_new` is the check: between looking and creating, someone else
        // may have made this name, and this is what refuses rather than truncates.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => {
                    IoError::new(409, "something with this name already exists")
                }
                _ => IoError::from(error),
            })?;
        Ok(FileEntry {
            name: rel
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: rel.to_string_lossy().replace('\\', "/"),
            kind: "file".into(),
            size: 0,
        })
    })
    .await
    .map_err(|error| IoError::new(500, format!("file create task failed: {error}")))?
}

/// Give an existing file or directory another name, or another place in this
/// workspace.
///
/// Both ends are checked the same way, because a rename is two paths and either
/// one leaving the workspace, naming protected state, or passing through a link
/// would be a way out of it. An occupied destination is refused rather than
/// replaced: renaming onto a name is not a request to destroy what holds it.
pub async fn rename_entry(root: &Path, from: &str, to: &str) -> Result<FileEntry, IoError> {
    let root = root.to_path_buf();
    let from = from.to_owned();
    let to = to.to_owned();
    tokio::task::spawn_blocking(move || {
        let canonical_root = fs::canonicalize(&root).map_err(IoError::from)?;
        let source = safe_relative(&from)?;
        let destination = safe_relative(&to)?;
        if source.as_os_str().is_empty() || destination.as_os_str().is_empty() {
            return Err(IoError::new(400, "both names are required"));
        }
        if is_protected_path(&source) || is_protected_path(&destination) {
            return Err(IoError::new(403, "this path is protected"));
        }
        if source == destination {
            return Err(IoError::new(400, "the name is unchanged"));
        }
        // The source's own final component may be a link -- renaming one is
        // renaming the link, not what it points at -- but a link anywhere above
        // either path would take the rename outside the workspace.
        if has_symlink_component(&canonical_root, source.parent().unwrap_or(Path::new("")))?
            || has_symlink_component(&canonical_root, &destination)?
        {
            return Err(IoError::new(403, "symlink writes are not allowed"));
        }
        let source_path = canonical_root.join(&source);
        let destination_path = canonical_root.join(&destination);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => IoError::new(404, "this file no longer exists"),
            _ => IoError::from(error),
        })?;
        if fs::symlink_metadata(&destination_path).is_ok() {
            return Err(IoError::new(409, "something with this name already exists"));
        }
        let parent = destination_path
            .parent()
            .ok_or_else(|| IoError::new(400, "invalid file path"))?;
        if !parent.is_dir() {
            return Err(IoError::new(404, "the destination folder does not exist"));
        }
        fs::rename(&source_path, &destination_path).map_err(IoError::from)?;
        Ok(FileEntry {
            name: destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: destination.to_string_lossy().replace('\\', "/"),
            kind: entry_kind(&metadata),
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
        })
    })
    .await
    .map_err(|error| IoError::new(500, format!("file rename task failed: {error}")))?
}

pub async fn delete_entry(root: &Path, relative: &str) -> Result<(), IoError> {
    let canonical = safe_path(root, relative)?;
    let canonical_root = fs::canonicalize(root).map_err(IoError::from)?;
    if canonical == canonical_root {
        return Err(IoError::new(400, "the workspace root cannot be deleted"));
    }
    if is_protected_path(&safe_relative(relative)?) {
        return Err(IoError::new(403, "this path is protected"));
    }
    let candidate = canonical_root.join(safe_relative(relative)?);
    let link = fs::symlink_metadata(&candidate)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    let target = if link { candidate } else { canonical };
    let metadata = fs::symlink_metadata(&target).map_err(IoError::from)?;
    if metadata.file_type().is_dir() {
        tokio::fs::remove_dir_all(&target)
            .await
            .map_err(IoError::from)
    } else {
        tokio::fs::remove_file(&target).await.map_err(IoError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    #[tokio::test]
    async fn a_new_file_is_only_ever_a_new_one_and_never_an_emptied_old_one() {
        let repo = TempRepo::new();
        let root = &repo.path;
        fs::write(root.join("kept.txt"), "precious").unwrap();

        let made = create_file(root, "notes.md").await.unwrap();
        assert_eq!(
            (made.path.as_str(), made.kind.as_str(), made.size),
            ("notes.md", "file", 0)
        );
        assert_eq!(fs::read_to_string(root.join("notes.md")).unwrap(), "");

        // Naming the path makes the folders it names.
        create_file(root, "docs/guide/start.md").await.unwrap();
        assert!(root.join("docs/guide/start.md").is_file());

        // A taken name is refused, so nothing here can empty a file that has
        // something in it.
        assert_eq!(create_file(root, "kept.txt").await.unwrap_err().status, 409);
        assert_eq!(
            fs::read_to_string(root.join("kept.txt")).unwrap(),
            "precious"
        );
        assert_eq!(create_file(root, "docs").await.unwrap_err().status, 409);

        // The workspace boundary and protected state hold here as everywhere.
        assert!(create_file(root, "../escape.txt").await.is_err());
        assert!(create_file(root, ".git/hooks/pre-commit").await.is_err());
        assert!(create_file(root, "").await.is_err());
    }

    #[tokio::test]
    async fn renaming_moves_a_name_without_replacing_what_already_holds_one() {
        let repo = TempRepo::new();
        let root = &repo.path;
        fs::write(root.join("draft.txt"), "body").unwrap();
        fs::write(root.join("taken.txt"), "someone else's").unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();

        let renamed = rename_entry(root, "draft.txt", "final.txt").await.unwrap();
        assert_eq!(
            (
                renamed.path.as_str(),
                renamed.name.as_str(),
                renamed.kind.as_str()
            ),
            ("final.txt", "final.txt", "file")
        );
        assert_eq!(fs::read_to_string(root.join("final.txt")).unwrap(), "body");
        assert!(!root.join("draft.txt").exists());

        // A path, not just a name: an existing folder is a valid destination.
        rename_entry(root, "final.txt", "docs/final.txt")
            .await
            .unwrap();
        assert!(root.join("docs/final.txt").is_file());
        // A folder goes with everything under it, and reports itself as one.
        let moved = rename_entry(root, "docs", "guide").await.unwrap();
        assert_eq!(moved.kind, "directory");
        assert!(root.join("guide/final.txt").is_file());

        // What already holds a name keeps it.
        assert_eq!(
            rename_entry(root, "guide/final.txt", "taken.txt")
                .await
                .unwrap_err()
                .status,
            409
        );
        assert_eq!(
            fs::read_to_string(root.join("taken.txt")).unwrap(),
            "someone else's"
        );
        // And the destination's folder has to exist, rather than being invented.
        assert_eq!(
            rename_entry(root, "taken.txt", "nowhere/taken.txt")
                .await
                .unwrap_err()
                .status,
            404
        );
        assert_eq!(
            rename_entry(root, "missing.txt", "anything.txt")
                .await
                .unwrap_err()
                .status,
            404
        );
        assert!(rename_entry(root, "taken.txt", "taken.txt").await.is_err());

        // Neither end may leave the workspace or touch protected state.
        assert!(
            rename_entry(root, "taken.txt", "../escaped.txt")
                .await
                .is_err()
        );
        assert!(
            rename_entry(root, "../outside.txt", "inside.txt")
                .await
                .is_err()
        );
        assert!(rename_entry(root, ".git", "history").await.is_err());
        assert!(
            rename_entry(root, "taken.txt", ".ssh/id_rsa")
                .await
                .is_err()
        );
        assert!(root.join(".git").exists());

        #[cfg(unix)]
        {
            // Renaming a link renames the link. Its target is not this
            // workspace's to move, and following it would move it anyway.
            let outside =
                std::env::temp_dir().join(format!("agentdock-rename-target-{}", Uuid::new_v4()));
            fs::write(&outside, "precious").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("outward")).unwrap();
            let link = rename_entry(root, "outward", "pointer").await.unwrap();
            assert_eq!(link.kind, "symlink");
            assert!(outside.exists(), "the link's target must not move");
            assert_eq!(fs::read_to_string(&outside).unwrap(), "precious");
            // But a link above either path is a way out of the workspace.
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("elsewhere")).unwrap();
            assert!(
                rename_entry(root, "taken.txt", "elsewhere/taken.txt")
                    .await
                    .is_err()
            );
            fs::remove_file(&outside).ok();
        }
    }

    #[tokio::test]
    async fn delete_removes_workspace_entries_but_not_the_root_or_what_a_link_points_at() {
        let repo = TempRepo::new();
        let root = &repo.path;
        fs::write(root.join("notes.txt"), "keep").unwrap();
        fs::create_dir_all(root.join("build/inner")).unwrap();
        fs::write(root.join("build/inner/out.bin"), "x").unwrap();

        delete_entry(root, "notes.txt").await.unwrap();
        assert!(!root.join("notes.txt").exists());
        // A directory goes with everything under it.
        delete_entry(root, "build").await.unwrap();
        assert!(!root.join("build").exists());

        // The root, protected names and anything outside stay untouchable.
        assert!(delete_entry(root, ".").await.is_err());
        assert!(delete_entry(root, ".git").await.is_err());
        assert!(delete_entry(root, "../escape").await.is_err());
        assert!(root.join(".git").exists());

        // Deleting a link unlinks it; following it would destroy the target,
        // which is the one thing a workspace delete must never do.
        #[cfg(unix)]
        {
            let outside =
                std::env::temp_dir().join(format!("agentdock-link-target-{}", Uuid::new_v4()));
            fs::write(&outside, "precious").unwrap();
            let inside = root.join("inside.txt");
            fs::write(&inside, "also precious").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("outward")).unwrap();
            std::os::unix::fs::symlink(&inside, root.join("inward")).unwrap();
            // A link out of the workspace resolves outside it and is refused.
            assert!(delete_entry(root, "outward").await.is_err());
            assert!(outside.exists());
            delete_entry(root, "inward").await.unwrap();
            assert!(!root.join("inward").exists());
            assert!(inside.exists(), "the link's target must survive");
            fs::remove_file(&outside).ok();
        }
    }

    struct TempRepo {
        path: PathBuf,
    }
    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("agentdock-test-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            for args in [
                vec!["init"],
                vec!["config", "user.email", "agentdock@example.com"],
                vec!["config", "user.name", "AgentDock"],
            ] {
                let mut cmd = StdCommand::new("git");
                cmd.args(args).current_dir(&path);
                assert!(cmd.output().unwrap().status.success());
            }
            Self { path }
        }
        fn git(&self, args: &[&str]) {
            assert!(
                StdCommand::new("git")
                    .args(args)
                    .current_dir(&self.path)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }
    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn file_roundtrip_and_conflict() {
        let repo = TempRepo::new();
        let file = write_file(&repo.path, "src/a.txt", "hello", None)
            .await
            .unwrap();
        assert_eq!(
            read_file(&repo.path, "src/a.txt").await.unwrap().version,
            file.version
        );
        let err = write_file(&repo.path, "src/a.txt", "changed", Some("v1:deadbeef"))
            .await
            .unwrap_err();
        assert_eq!(err.status, 409);
        write_file(&repo.path, "src/a.txt", "changed", Some(&file.version))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn git_status_rename_deleted_untracked() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("a.txt"), "a").unwrap();
        repo.git(&["add", "a.txt"]);
        repo.git(&["commit", "-m", "init"]);
        fs::rename(repo.path.join("a.txt"), repo.path.join("b.txt")).unwrap();
        fs::write(repo.path.join("new.txt"), "new").unwrap();
        let status = git_status(&repo.path).await.unwrap();
        assert!(
            status
                .files
                .iter()
                .any(|f| f.path == "b.txt" || f.original_path.as_deref() == Some("a.txt"))
        );
        assert!(status.files.iter().any(|f| f.path == "new.txt"));
    }

    #[tokio::test]
    async fn stage_diff_and_unstage() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("new.txt"), "hello\n").unwrap();
        let diff = git_diff(&repo.path, None, false).await.unwrap();
        assert!(diff.diff.contains("new.txt"));
        git_stage(&repo.path, &["new.txt".into()]).await.unwrap();
        assert!(
            git_diff(&repo.path, None, true)
                .await
                .unwrap()
                .diff
                .contains("new.txt")
        );
        git_unstage(&repo.path, &["new.txt".into()]).await.unwrap();
    }

    #[tokio::test]
    async fn branches_switch_and_worktrees_sit_beside_the_repository() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("a.txt"), "a\n").unwrap();
        git_stage(&repo.path, &["a.txt".into()]).await.unwrap();
        git_commit(&repo.path, "first").await.unwrap();
        let start = git_branches(&repo.path).await.unwrap();
        let home = start.current.clone().expect("on a branch");
        assert_eq!(start.worktrees.len(), 1);
        assert!(start.worktrees[0].main);

        git_switch(&repo.path, "feature/x", true).await.unwrap();
        assert_eq!(
            git_branches(&repo.path).await.unwrap().current.as_deref(),
            Some("feature/x")
        );
        git_switch(&repo.path, &home, false).await.unwrap();
        assert_eq!(
            git_switch(&repo.path, "-rf", false)
                .await
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            git_switch(&repo.path, "no-such-branch", false)
                .await
                .unwrap_err()
                .status,
            409
        );

        let tree = git_worktree_add(&repo.path, "feature/y", true)
            .await
            .unwrap();
        let canonical = fs::canonicalize(&repo.path).unwrap();
        let expected = canonical.parent().unwrap().join(format!(
            "{}.worktrees",
            canonical.file_name().unwrap().to_string_lossy()
        ));
        assert_eq!(tree, expected.join("feature-y"));
        assert!(tree.join("a.txt").exists());
        let after = git_branches(&repo.path).await.unwrap();
        assert!(
            after
                .worktrees
                .iter()
                .any(|w| !w.main && w.branch.as_deref() == Some("feature/y"))
        );
        // The same branch cannot get a second directory.
        assert_eq!(
            git_worktree_add(&repo.path, "feature/y", false)
                .await
                .unwrap_err()
                .status,
            409
        );
        let _ = fs::remove_dir_all(&expected);
    }

    #[tokio::test]
    async fn branches_rename_and_delete_and_worktrees_are_removed_but_never_the_main_one() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("a.txt"), "a\n").unwrap();
        git_stage(&repo.path, &["a.txt".into()]).await.unwrap();
        git_commit(&repo.path, "first").await.unwrap();
        let tree = git_worktree_add(&repo.path, "side", true).await.unwrap();
        let tree_text = tree.to_string_lossy().into_owned();
        // A rename is followed by the worktree that holds the branch.
        git_branch_rename(&repo.path, "side", "renamed")
            .await
            .unwrap();
        let listed = git_branches(&repo.path).await.unwrap();
        assert!(
            listed
                .worktrees
                .iter()
                .any(|w| w.branch.as_deref() == Some("renamed"))
        );
        // A branch checked out somewhere cannot be deleted.
        assert_eq!(
            git_branch_delete(&repo.path, "renamed", true)
                .await
                .unwrap_err()
                .status,
            409
        );
        // Uncommitted work keeps a worktree unless forced.
        fs::write(tree.join("dirty.txt"), "x").unwrap();
        assert_eq!(
            git_worktree_remove(&repo.path, &tree_text, false)
                .await
                .unwrap_err()
                .status,
            409
        );
        git_worktree_remove(&repo.path, &tree_text, true)
            .await
            .unwrap();
        assert!(!tree.exists());
        git_branch_delete(&repo.path, "renamed", false)
            .await
            .unwrap();
        assert!(
            !git_branches(&repo.path)
                .await
                .unwrap()
                .branches
                .contains(&"renamed".to_owned())
        );
        let main = fs::canonicalize(&repo.path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            git_worktree_remove(&repo.path, &main, true)
                .await
                .unwrap_err()
                .status,
            400
        );
        let _ = fs::remove_dir_all(tree.parent().unwrap());
    }

    #[tokio::test]
    async fn discarding_restores_what_is_staged_and_removes_what_git_never_knew() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("kept.txt"), "one\n").unwrap();
        git_stage(&repo.path, &["kept.txt".into()]).await.unwrap();
        git_commit(&repo.path, "first").await.unwrap();
        // Staged work survives: discard goes back to the index, not to HEAD.
        fs::write(repo.path.join("kept.txt"), "two\n").unwrap();
        git_stage(&repo.path, &["kept.txt".into()]).await.unwrap();
        fs::write(repo.path.join("kept.txt"), "three\n").unwrap();
        fs::write(repo.path.join("scratch.txt"), "junk\n").unwrap();
        git_discard(&repo.path, &["kept.txt".into(), "scratch.txt".into()])
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(repo.path.join("kept.txt")).unwrap(),
            "two\n"
        );
        assert!(!repo.path.join("scratch.txt").exists());
        // A deleted tracked file comes back.
        fs::remove_file(repo.path.join("kept.txt")).unwrap();
        git_discard(&repo.path, &["kept.txt".into()]).await.unwrap();
        assert!(repo.path.join("kept.txt").exists());
        // Nothing is discarded without being named.
        assert_eq!(
            git_discard(&repo.path, &[".".into()])
                .await
                .unwrap_err()
                .status,
            400
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_is_rejected() {
        let repo = TempRepo::new();
        let outside = std::env::temp_dir().join(format!("agentdock-outside-{}", Uuid::new_v4()));
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, repo.path.join("link.txt")).unwrap();
        assert!(safe_path(&repo.path, "link.txt").is_err());
        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_ancestor_cannot_be_written() {
        let repo = TempRepo::new();
        let real = repo.path.join("real");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, repo.path.join("alias")).unwrap();
        let err = write_file(&repo.path, "alias/file.txt", "nope", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 403);
    }
}
