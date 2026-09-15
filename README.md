# AgentDock

A host-native workspace for official Claude Code / Codex clients, built with Rust + Vue 3 + TypeScript. Light UI, recursive split/tab docking, first-class Git review, files and optional terminals. No Docker requirement.

## Run locally (macOS / Linux)

Requires Rust stable (1.89+), **Node.js 24+** and **pnpm 10.30.3**. `.nvmrc` / `.node-version` and CI use Node 24; an installed newer Node is also supported by the project engine range. Install Claude Code and/or Codex separately.

```sh
pnpm install --frozen-lockfile
pnpm run build:web
cargo run -p agentdock-server
```

Open **http://127.0.0.1:8787/**. The Rust server serves the built Web client and API on the same origin.

For development, run `pnpm run dev` to start Cargo + Vite, then open **http://127.0.0.1:5173/**. If Rust is already running, use only `pnpm run dev:web`. Vite proxies `/api` and WebSockets to Rust. pnpm manages the workspace and dependencies; Vite, Vue SFC typechecking, tests and the native-history bridge run under Node. Rust builds still use Cargo.

The previous Bun lockfile and version pin are retained under `docs/toolchain-history/` as historical evidence, not an active toolchain. Do not use them for current installs. In the observed macOS debugging case, the backend WebSocket opened directly but timed out through Bun-run Vite; running Vite under Node restored the proxy handshake. The existing development frontend on 5173 was switched to Node without restarting the running backend on 8787 or its Agent processes.

Choose an existing host directory in the workspace picker, then create/open a session. New Web-created Agent sessions use structured conversation cards on a capable backend; existing PTY sessions keep their original interface. Tool approvals remain native decisions, rendered by the Web client. **Interrupt** cancels current work, while **End session** remains a separate, confirmed secondary action. Closing a pane never ends its process.

## What works in the MVP

Current implementation includes Chinese/English UI, the shared split/tab canvas, native structured conversation adapters, account/configuration management and first-class file/Git panes. Implementation and fixture coverage do not mean every live provider/account flow has been exercised; deployment-specific verification stays in [the acceptance record](docs/mvp-status.md).

- Recursive horizontal/vertical splits, drag-to-edge docking, center-drop tabs, resize/snap, 1:1 / 2×2 / 1:2:1 presets, maximize and restore.
- Below 280×180, panes become tabs without overwriting the saved layout. Sessions and in-memory file/commit drafts survive view remounts.
- Workspace registry and fuzzy-searchable global sessions; opening connects, while ending remains an explicit secondary action. Refreshing a stopped session does not restart it.
- Two-level workspace → session navigation, with one shared canvas across projects. Expanding/selecting a workspace does not replace existing panes. Files and Git actions stay bound to each pane's workspace; the explorer follows focus or can be pinned.
- **Load existing session** lists native Codex/Claude Code history belonging to the chosen workspace, supports fuzzy search, and links a selected history after confirmation. It resumes history, not an already-running external terminal.
- Structured Agent messages, folding tool cards, native approval/question cards, usage and turn status, with responsive controls for narrow layouts. Rust supervises a bounded Node JSONL bridge: Codex uses official `app-server`; Claude Code uses the user's existing CLI `stream-json` control interface. Neither adapter implements an Agent loop.
- Claude Code/Codex legacy PTY sessions and optional host-shell terminals remain available. A live PTY is never silently taken over by structured chat. Disconnect/reconnect preserves its running process.
- Confirmed, same-client endpoint/configuration switching while idle: close the old bridge, use a fresh native conversation and configuration generation, and retain earlier UI history without forwarding it or the old environment to the new endpoint.
- Process state and stream connection state are displayed separately. An 8-second WebSocket handshake watchdog reports an actionable connection failure instead of staying “connecting”; it does not automatically retry, restart a process, or replay user input.
- Custom profiles keep per-session endpoint snapshots, model and permission-intent mapping. Profile edits affect future sessions only.
- Advanced environment editors for profile defaults, new sessions, history loading and existing sessions. Host environment inheritance remains process-local; explicit overrides are validated and stored separately.
- **Import existing configuration** adds a reusable reference to the host's existing Codex/Claude Code home. New sessions can reuse its login, model, endpoint and native settings without copying credentials. This shared reference is explicitly different from an isolated custom profile.
- Profiles can fetch model IDs explicitly: native Codex uses app-server `model/list`; custom endpoints use the read-only models API. Manual model IDs/aliases are always available. Account access can differ from catalogue visibility.
- A new isolated/custom-profile session can override its model without changing its profile or other sessions. A host-reference session keeps its native model settings; an optional advanced process-environment overlay never writes back to the source configuration.
- Lazy expandable file tree with keyboard navigation and per-workspace remembered expansion, plus **Reveal in file tree** on previews. Search filters loaded entries, without recursively scanning the host.
- Text edit/save with version conflicts, image/video/audio/PDF browser previews and download.
- First-class Git status/diff, staged/unstaged groups, file/all staging, unstaging and explicit commit.
- SQLite WAL metadata, schema 9, persisted layouts and sequenced conversation events. The recent transcript window is bounded; control anchors and pending approvals survive display-window trimming. UUID/SHA-256 submission receipts prevent replay across reconnects/restarts.
- Official-account records with private native config directories. Codex browser/device login, cancellation, token refresh, logout, available official quota information and eligible earned reset credits use the official client APIs. Claude login remains under the official client; usage is an explicit read of the official OAuth usage endpoint, while login/token refresh and quota reset remain unavailable. See [official account boundaries](docs/official-accounts.md).
- Local Host/Origin/CSRF checks; optional access-token login and HttpOnly cookie for private remote use.

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

### Structured conversations and retained history

The Web client requests `interaction_mode: "structured"` for new Agent conversations when `structured_chat` is advertised. Omitting the field in the HTTP API still defaults to `pty` for older callers. A stopped legacy Agent session may explicitly opt into chat; a running PTY must first be ended by its user. Ordinary view remounts do not restart stopped sessions.

Chat initialization does not inject a prompt. Codex starts/resumes its thread only after an explicit message; Claude uses its installed native CLI and existing settings rather than an SDK OAuth/login proxy. Native startup hooks/settings remain native behavior. Tool permission defaults are not bypassed; unsupported interactions fail closed. Claude's noninteractive `--print` mode skips its interactive workspace-trust dialog, so use registered directories you trust.

SQLite schema 9 assigns monotonic event sequences, stores endpoint reasoning defaults, and retains a recent 2,000-event / 8 MiB display window, plus the latest control anchors and at most 32 unresolved approvals. These extra anchors mean a snapshot can exceed the recent window's event count. Submission receipts contain message UUIDs and content SHA-256 hashes rather than another full prompt copy; receipts are not discarded merely because visible history was trimmed. Input is persisted before dispatch and is never automatically replayed after an uncertain outcome.

The Node bridge limits text fields to 64 KiB, marking truncated text, and serialized output lines to 192 KiB; the Rust receiver also enforces its own frame limits. Reconnection restores stored events, not a lost live process. **Imported native history currently supplies resume context to the CLI; its older full transcript is not backfilled into the structured Web view.** Do not mistake an empty imported Web transcript for a newly empty native conversation.

See [structured UI implementation and limitations](docs/structured-agent-ui.md) for the protocol and configuration-switch semantics.

### Advanced environment configuration

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
AGENTDOCK_ADDR                # default 127.0.0.1:8787
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
AGENTDOCK_INSTANCE_LABEL      # optional UI label for isolated preview deployments
```

## Private remote access

Simplest: keep loopback binding and use SSH port forwarding, `ssh -L 8787:127.0.0.1:8787 user@server`.

For a network bind, configure `AGENTDOCK_TOKEN` (24+ random characters), `AGENTDOCK_ALLOWED_ORIGINS=https://your-host`, and an HTTPS reverse proxy that preserves Host and WebSocket upgrades. Then sign in using the token in the Web UI. Do not expose this trusted-host workspace to untrusted users.

## Boundaries

This is a **single trusted user's host workspace**, not a multi-tenant sandbox. Native tool permissions remain native. A working-directory check or config directory is not an OS isolation boundary.

- Browser disconnection preserves processes; server shutdown stops managed processes. Restart marks old running records stopped. It does not revive a lost PTY.
- State and database ownership locks prevent two new-version servers from managing the same sessions. Port binding and ownership checks happen before migration/reconciliation. `init` also respects these locks and never resets running-session status. Keep state on a local filesystem with working OS file locks; do not remove lock files while a process is alive.
- Terminal replay is the most recent 1 MiB in memory, not a durable transcript. After a server restart use the native client's history/resume workflow.
- Model-generation and paid API requests are not part of the automated tests; sign in or supply your own provider reference.
- No custom Agent reasoning loop, token-exchange server, provider reverse proxy, Keychain editor, automatic account import, strong cross-process credential isolation, hunk staging, Git network/PR workflow, LSP or computer-desktop control in this MVP.
- Media playback depends on browser codecs. Text editing limit: 2 MiB. Large Git output is rejected; no silent truncation.
- Common proxy support is HTTP/HTTPS without inline passwords. Claude Code does not support SOCKS. Empty proxy follows host settings; explicit profiles override inherited proxy variables for that process only.
- Model listing never performs inference. HTTP requests have time/body limits and do not follow redirects with credentials; non-compatible endpoints keep manual model entry as fallback.
- Resource guardrails bound layout depth/nodes, event windows and runtime replay. Managed process groups receive bounded cleanup; deliberately detached daemons remain outside that group boundary.
- macOS has been run locally. Linux has a CI target, not a claim of a completed deployment here.

## Verify

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm run check
pnpm run build:web
```

See [API contract](docs/api.md) and [acceptance record](docs/mvp-status.md). Earlier design documents describe the broader roadmap, not finished features.

### Read-only UI preview and deployment caution

With the development frontend running, `http://127.0.0.1:5173/ui-preview.html` renders the actual UI components against clearly labeled local snapshots. It does not call the user's Agent backend. Its 390px container previews narrow-screen layout; it is **not** a real-phone or touch-device acceptance test.

The existing 5173/8787 development installation has four user-owned active sessions; do not restart its backend or end those sessions merely to expose new capabilities. Use an explicitly separate state/database/port for feature previews (8788 is the designated preview port), and record what was actually verified there. Do not infer live OAuth success, a paid model turn, Linux deployment or mobile-device acceptance from compiled code, fixtures or the read-only preview.

## Shared workspace canvas

The sidebar groups sessions directly under expandable workspaces. File, Git and new-session shortcuts belong to each workspace; provider/account/branch information does not add extra navigation levels. Sessions from any workspace can be opened or dragged into the same split/tab layout. Workspace selection only changes the navigation/new-session target, not the existing canvas.

Every bound pane includes an explicit workspace ID. File IDs include both workspace and relative path, so two `README.md` files cannot overwrite each other's view. Each Git pane has a fixed repository; changing sidebar selection never redirects staging or commits. Pane headers show the workspace, and the explorer can follow focus or stay pinned. Layout manipulation still never terminates an Agent.

New servers advertise `shared_canvas` through `/api/health` and store one revisioned layout in the singleton introduced by SQLite schema 5. Concurrent saves use compare-and-swap; a conflict keeps the current local copy and offers loading the server layout or explicitly saving the current one. First use seeds the shared canvas from the previously selected workspace's legacy layout when no shared/local layout exists. The old per-workspace layouts are never overwritten or deleted by this migration.

On an older running backend, the UI remains usable without restarting its sessions: the shared canvas is saved in this browser, visibly labeled as such, scoped by origin and the oldest workspace registration. It does **not** issue unsupported shared-canvas requests or overwrite a project's old layout. When local storage is unavailable the UI says **Memory only**. An updated backend is still required for native-history loading, configuration import, folder/model discovery and environment editing; unsupported controls are disabled with an upgrade explanation. Refresh the page after a normal backend upgrade. UI layout caches contain pane placement/identifiers, not file bodies, native terminal output or credentials.
