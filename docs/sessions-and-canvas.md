# Sessions and the shared canvas

How structured conversations are stored and resumed, and how one canvas is shared across workspaces. Back to the [README](../README.md).

## Structured conversations and retained history

The Web client requests `interaction_mode: "structured"` for new Agent conversations when `structured_chat` is advertised. Omitting the field in the HTTP API still defaults to `pty` for older callers. A stopped legacy Agent session may explicitly opt into chat; a running PTY must first be ended by its user. Ordinary view remounts do not restart stopped sessions.

Chat initialization does not inject a prompt. Codex starts/resumes its thread only after an explicit message; Claude uses its installed native CLI and existing settings rather than an SDK OAuth/login proxy. Native startup hooks/settings remain native behavior. Tool permission defaults are not bypassed; unsupported interactions fail closed. Claude's noninteractive `--print` mode skips its interactive workspace-trust dialog, so use registered directories you trust.

SQLite schema 9 assigns monotonic event sequences, stores endpoint reasoning defaults, and retains a recent 2,000-event / 8 MiB display window, plus the latest control anchors and at most 32 unresolved approvals. These extra anchors mean a snapshot can exceed the recent window's event count. Submission receipts contain message UUIDs and content SHA-256 hashes rather than another full prompt copy; receipts are not discarded merely because visible history was trimmed. Input is persisted before dispatch and is never automatically replayed after an uncertain outcome.

The Node bridge limits text fields to 64 KiB, marking truncated text, and serialized output lines to 192 KiB; the Rust receiver also enforces its own frame limits. Reconnection restores stored events, not a lost live process. **Imported native history currently supplies resume context to the CLI; its older full transcript is not backfilled into the structured Web view.** Do not mistake an empty imported Web transcript for a newly empty native conversation.

See [structured UI implementation and limitations](structured-agent-ui.md) for the protocol and configuration-switch semantics.

## Shared workspace canvas

The sidebar groups sessions directly under expandable workspaces. File, Git and new-session shortcuts belong to each workspace; provider/account/branch information does not add extra navigation levels. Sessions from any workspace can be opened or dragged into the same split/tab layout. Workspace selection only changes the navigation/new-session target, not the existing canvas.

Every bound pane includes an explicit workspace ID. File IDs include both workspace and relative path, so two `README.md` files cannot overwrite each other's view. Each Git pane has a fixed repository; changing sidebar selection never redirects staging or commits. Pane headers show the workspace, and the explorer can follow focus or stay pinned. Layout manipulation still never terminates an Agent.

New servers advertise `shared_canvas` through `/api/health` and store one revisioned layout in the singleton introduced by SQLite schema 5. Concurrent saves use compare-and-swap; a conflict keeps the current local copy and offers loading the server layout or explicitly saving the current one. First use seeds the shared canvas from the previously selected workspace's legacy layout when no shared/local layout exists. The old per-workspace layouts are never overwritten or deleted by this migration.

On an older running backend, the UI remains usable without restarting its sessions: the shared canvas is saved in this browser, visibly labeled as such, scoped by origin and the oldest workspace registration. It does **not** issue unsupported shared-canvas requests or overwrite a project's old layout. When local storage is unavailable the UI says **Memory only**. An updated backend is still required for native-history loading, configuration import, folder/model discovery and environment editing; unsupported controls are disabled with an upgrade explanation. Refresh the page after a normal backend upgrade. UI layout caches contain pane placement/identifiers, not file bodies, native terminal output or credentials.
