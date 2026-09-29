# Agent tools

Claude Code and Codex sessions can look at AgentDock and, with your say, configure it: set up an endpoint, change new-session defaults, add a workspace, start another session in parallel, or point you at a file. Back to the [README](../README.md).

## How agents reach it

Every Claude Code and Codex session AgentDock launches carries an `agentdock` MCP server, added through that launch's own flags (`--mcp-config` for Claude, `-c mcp_servers.agentdock.*` for Codex). Your `~/.claude`, `~/.codex` and any host configuration a profile points at are never written. Terminals get only the environment variables below.

The MCP server is `agentdock mcp`, a stdio relay: it forwards tool calls to the running AgentDock server, which decides everything. To use the same tools from a client you run yourself, outside AgentDock:

```sh
claude mcp add agentdock -- agentdock mcp
codex mcp add agentdock -- agentdock mcp
```

Such a caller is *external*: it has no session of its own, so the session tools below are not available to it.

Each launch also gets `AGENTDOCK_URL`, `AGENTDOCK_SESSION_ID` and its own `AGENTDOCK_AGENT_TOKEN`. The token is good only while that session's process runs, and only for `/api/agent/`.

Turn it off for every session under **Settings → Preferences → AgentDock tools for agents**, or for one session from its **⋯** menu (**AgentDock tools: Default / On / Off**; one session's choice wins over the preference). Either applies from the session's next start, when the tools are injected. A session cannot change its own setting: the switch is not reachable with its agent token.

## Tools

| Tool | What it does | Asks you first |
| --- | --- | --- |
| `agentdock_status` | Which session, workspace, directory and endpoint is calling | — |
| `agentdock_list` | Workspaces, sessions (metadata only), endpoint profiles (whether a key is set, never the key), installed clients | — |
| `agentdock_endpoint` | Create, change, delete, test, or import a host configuration as an endpoint profile | Yes, except `test` |
| `agentdock_preferences` | New-session defaults per client: endpoint, effort, permission | Only a switch to running tools without asking |
| `agentdock_workspace` | Add a directory as a workspace | — |
| `agentdock_spawn` | Start a Claude Code or Codex session with a first message, optionally on its own branch | The first time per session |
| `agentdock_session` | Follow sessions it started (`list`, `result`, `wait`, `message`); rename or keep its own | — |
| `agentdock_show` | Open a file at a line, the Git changes, or a session in your window | — |
| `agentdock_usage` | The quota its official account last reported | — |
| `agentdock_canvas` | List the tabs on the canvas; close some or all | Only when it would discard a temporary session still working |

## What you see

A change that asks you first appears in the AgentDock window as a card: who is asking, what would change, and **Approve** / **Decline**. The call waits up to five minutes for your answer; with no AgentDock window open it fails at once instead.

When an endpoint needs an API key, the card has a key field. The key goes from that field to AgentDock's key store (see [Configuration](configuration.md#state-and-credentials)) and never through the agent or its conversation. Agents are told never to ask for a key in chat.

Everything an agent changes without asking is shown as a notice, with **Undo** for endpoint profiles and defaults, and **Open** for a session it started.

## Limits

- A session can read and message only the sessions it started, and a session started this way cannot start more. At most four it started may run at once. Started sessions are temporary unless the agent asks to keep them.
- Agents cannot answer or even see the cards: those live under `/api/agent-activity/`, which an agent token does not open.
- Agents cannot read other sessions' conversations, approve their own tool calls, change their own permission mode, or sign accounts in or out.

The token identifies the caller; it is not the security boundary. On a deployment without an access token (the local default), any process running as you can already call AgentDock's whole API. What protects the changes that matter is the confirmation in your window.
