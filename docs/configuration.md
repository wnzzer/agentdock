# Configuration

State directory, credentials, host-configuration reuse, per-session environment and every `AGENTDOCK_*` variable. Back to the [README](../README.md).

## State and credentials

New installations initialize state at **`~/.agentdock/`**, independently of project directories. Run **`pnpm run init`** (not `pnpm init`, which is pnpm's package-initialization command), or `agentdock-server init` for a built binary; first server startup can also initialize it. This creates state only, not a system service or a downloaded installation.

Path priority: `AGENTDOCK_STATE_DIR` → `AGENTDOCK_HOME` → an existing `./.agentdock/agentdock.db` for legacy compatibility → `~/.agentdock`. Existing local data is never silently moved. Set `AGENTDOCK_HOME` explicitly to use the home directory while running from an older checkout. Keep the selected state directory between restarts:

```text
~/.agentdock/
├── agentdock.db
├── sessions/<session-id>/   # generated native CLI configuration/history
│   └── configurations/<revision>/ # fresh isolated home after confirmed configuration switches
└── accounts/<account-id>/   # private manifest and native-owned account configuration
```

Configuration directories are 0700, generated Codex config files are 0600. The native client manages its login cache. These paths can contain credentials; don't commit/share them. Use SQLite Online Backup for the DB and protect backups of native state separately.

API-key profiles currently use a **reference**, e.g. `env:AGENTDOCK_SECRET_WORK`. Set that environment variable for the server; never enter the key into the profile form. AgentDock resolves the selected reference on the backend and injects its value into the selected child-process destination variable.

New sessions without a host-reference profile use separate generated native configuration directories. **Loaded native histories are different:** resuming uses the original client configuration/login/history directory, after explicit confirmation. These loaded sessions share their source account/configuration, do not copy credentials into AgentDock, and do not add endpoint/model/permission-policy overrides. Structured mode supplies the required stdio transport flags separately. Explicit advanced environment overrides are optional. Metadata listing uses official SDK/app-server APIs, makes no model request, returns at most 500 workspace-scoped items, and reports truncation. Missing/incompatible native clients produce an actionable error rather than scraping terminal output.

### Reuse the host's existing configuration

In **Endpoint profiles → Import existing configuration**, choose the host's Codex or Claude Code source, name it, and confirm shared configuration. Then choose that profile when creating a new session. Repeated imports return the same profile instead of duplicating it.

Importing also selects that profile for future new sessions of this client in the current browser. New-session dialogs remember the last successfully used configuration per provider. Without a saved choice, one imported host profile is preselected; multiple imported profiles require an explicit choice. An explicitly chosen isolated configuration remains available and is remembered. Refreshing the profile list never silently replaces a valid account already displayed in an open form.

Importing **does not rebind existing sessions**. If a session still asks for login, check its configuration badge: an isolated session uses a separate login environment even when a host profile exists. Create a new session with the imported host profile to reuse its valid sign-in. The UI now distinguishes this case explicitly.

- Discovery checks directory availability only. It does not inspect credential contents, claim that login is valid, launch a client, or perform inference.
- Source resolution follows the same deployment settings as native history: `AGENTDOCK_CODEX_HISTORY_DIR` → `CODEX_HOME` → `~/.codex`, and `AGENTDOCK_CLAUDE_HISTORY_DIR` → `CLAUDE_CONFIG_DIR` → `~/.claude`. These are the **server user's** directories, not the browser machine's.
- A reference pins the provider, source ID, canonical directory and original directory-environment context. A changed/deleted source is rejected at create/start. The **directory's contents remain live**, not an immutable copy; native configuration changes can affect subsequent starts.
- The original unset-versus-explicit `CODEX_HOME`/`CLAUDE_CONFIG_DIR` state is preserved. On macOS, explicitly setting even the default Claude directory can select a different Keychain login context. Explicit absolute paths keep their original spelling; relative directory variables are rejected because the workspace changes the child's working directory.
- AgentDock neither creates a new config home nor rewrites the source settings. The native client still manages its own credential refresh and conversation history. Existing login is reused when valid; reauthorization can still be required by the provider.
- Host-reference profiles support rename, advanced environment defaults and removal of the reference. Direct endpoint/model/proxy/permission fields and the native source identity remain read-only. Removing one never deletes the native home, logs out the account, or removes snapshots from existing sessions.
- Native clients inherit the backend process's environment, excluding AgentDock's private secret namespace/access token and required native-context removals. AgentDock does not source shell startup files or import aliases, shell functions, temporary exports or flags from another terminal. There is no generic import/conversion of CLIProxyAPI token JSON.

This is an explicit **shared native configuration reference**, not a copied account or a claim of process-level credential isolation. The separate official-account manager creates its own private account directories; imported host references retain their existing source directory. Each new session gets its own process/conversation; loading a specific old conversation remains **Load existing session**. Managed account/profile ownership must not be broken by deleting its profile independently.

## Settings file

Everything a deployment decides — whether it is reachable from other machines, on which port, with which token — lives in **`<state>/config.toml`**, so it is written once rather than remembered as a handful of variables:

```toml
lan = true              # reachable from other machines (same as --lan)
port = 28789            # or addr = "192.168.0.9:28789"
token = "..."           # AGENTDOCK_TOKEN
token_min = 24          # shortest token this deployment accepts
instance_label = "novel box"
allowed-origins = ["http://box.local:28789"]
```

Any other key names the `AGENTDOCK_` variable it spells (`shell`, `claude-bin`, `browse-roots`, …); dashes and underscores are the same, a full `AGENTDOCK_*` name is accepted as written, and a list becomes the comma-separated form those variables already take. Keys always land under that prefix, so this file cannot set a variable outside AgentDock's namespace.

Precedence is command line → environment → file → default. `--lan` outranks the file only about the host: a port written down is still the port that was meant. `AGENTDOCK_HOME` and `AGENTDOCK_STATE_DIR` cannot come from the file, which lives inside the directory they choose.

## Advanced environment configuration

Expand **Advanced environment** in an endpoint profile, new-session form or native-history loader. To inspect an existing session without launching it, use its sidebar settings gear or the pane's session-actions menu. A running session is read-only here; only `stopped` / `failed` sessions can save changes. Saving never starts, stops or restarts a process. Changes apply at its next explicit launch.

- Profile environment defaults are merged with new-session overrides and snapshotted into that session's effective explicit map. Later profile edits do not change existing sessions. Editing an existing session **replaces the entire effective map**, not a partial merge.
- History loading with no overrides omits the environment field and preserves an already-imported session's map. Repeating an import with different explicit overrides returns a conflict (409); it never silently changes a running session.
- The inherited backend environment is not copied into SQLite or exposed by the editor. Another terminal's exports, shell startup configuration and an old session's temporary environment cannot be reconstructed from history. Native configuration files retain the CLI's own precedence; an environment overlay does not rewrite those files or promise to override every native setting.
- **Literal** assigns the exact value: `$PATH`, `${NAME}`, `~` and command-looking text are not shell-expanded. Literal values are plaintext local metadata, so never enter credentials there. **Secret reference** accepts only `env:AGENTDOCK_SECRET_NAME`; configure that variable for the backend and it is resolved only at launch, never returned as an editor value. **Unset** explicitly removes a child-process variable.
- Deleting an override is not the same as **Unset**. In a new-session draft it reveals any profile default; in an existing session's replacement map it drops that explicit value and leaves the base launch environment to apply. Native config-file settings still follow native-client behavior.
- Names use `[A-Za-z_][A-Za-z0-9_]{0,127}`. `HOME`, `USERPROFILE`, `PWD`, `OLDPWD`, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `CLAUDECODE` and all `AGENTDOCK_*` names are reserved, case-insensitively. Names containing `TOKEN`, `SECRET`, `PASSWORD`, `PRIVATE_KEY`, `API_KEY` or `AUTHORIZATION`, and proxy/base URLs containing user information, require a secret reference instead of a literal value (or may be unset).
- Limits: 64 explicit entries and 64 KiB of serialized JSON; literal and resolved secret values are at most 8192 UTF-8 bytes and cannot contain NUL. Profile defaults and per-session overrides must also fit the limits after merging.

An optional interactive **Terminal** still follows its shell's own startup-file behavior; that does not make those files or an old terminal's exports an imported native-session snapshot.

SQLite schema 6 adds explicit environment maps with empty defaults, preserving older records and snapshots. The backend advertises `session_environment` in `/api/health`; editors are disabled with an upgrade explanation when it is absent. Updating frontend files alone does not enable this API on an already-running old backend. Upgrade that backend through an orderly restart after saving/ending its active native sessions; do not interrupt them merely to enable the editor.

```text
AGENTDOCK_DB                  # optional database path
AGENTDOCK_HOME                # installation/state home; default ~/.agentdock for new installs
AGENTDOCK_STATE_DIR           # optional persistent state directory
AGENTDOCK_ADDR                # default 127.0.0.1:28789
AGENTDOCK_TOKEN               # access token; required by a binding that reaches other machines
AGENTDOCK_TOKEN_MIN           # shortest accepted token; default 24
AGENTDOCK_ALLOWED_ORIGINS     # optional extra browser origins, comma-separated
AGENTDOCK_WEB_DIR             # state-dir/web if installed, else source apps/web/dist
AGENTDOCK_NATIVE_BRIDGE       # optional path to packages/native-bridge/history.mjs
AGENTDOCK_JS_RUNTIME          # native helper/chat/account runtime; default node (absolute path supported)
AGENTDOCK_CHAT_BRIDGE         # optional path to packages/native-bridge/chat.mjs
AGENTDOCK_ACCOUNT_BRIDGE      # optional path to packages/native-bridge/account.mjs
AGENTDOCK_NODE_BIN            # legacy runtime override, used only if JS_RUNTIME is unset/empty
AGENTDOCK_CODEX_HISTORY_DIR   # optional history source; otherwise CODEX_HOME or ~/.codex
AGENTDOCK_CLAUDE_HISTORY_DIR  # optional history source; otherwise CLAUDE_CONFIG_DIR or ~/.claude
AGENTDOCK_CLAUDE_BIN          # optional absolute path to claude
AGENTDOCK_CODEX_BIN           # optional absolute path to codex
AGENTDOCK_SHELL               # optional shell; defaults to SHELL or /bin/sh
AGENTDOCK_BROWSE_ROOTS        # optional directory-picker roots, OS path-list (macOS/Linux colon-separated)
AGENTDOCK_WORKSPACE_ROOTS     # optional roots a new workspace may sit under; defaults to the browsing roots
AGENTDOCK_INSTANCE_LABEL      # optional UI label for isolated preview deployments
```
