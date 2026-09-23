export type ProviderKind = "claude_code" | "codex" | "terminal";
export type AgentProviderKind = Exclude<ProviderKind, "terminal">;
export type SessionStatus = "starting" | "running" | "waiting" | "stopped" | "failed";
export type EnvironmentValue = { kind: "literal"; value: string } | { kind: "secret_ref"; reference: string } | { kind: "unset" };
export type EnvironmentOverrides = Record<string, EnvironmentValue>;

export interface Workspace {
  id: string;
  name: string;
  root_path: string;
  created_at: string;
}

export interface Session {
  interaction_mode?: "pty" | "structured";
  configuration_revision?: number;
  environment?: EnvironmentOverrides;
  id: string;
  workspace_id: string;
  provider: ProviderKind;
  title: string;
  status: SessionStatus;
  created_at: string;
  updated_at: string;
  /** Reversible list organization, independent of running status and history. */
  archived_at?: string | null;
  /** Chosen once at creation: closing its window discards the record and stops
   * its process. Only ever cleared (promoted), never set on an existing session. */
  ephemeral?: boolean;
  endpoint_profile_id: string | null;
  endpoint_snapshot?: EndpointProfile | null;
  provider_session_id: string | null;
  native_source_id?: string | null;
  /** Set on a terminal opened as an escape hatch out of a structured session:
   * the id of the session whose conversation it reopens. Such a session runs
   * the client interactively on the same conversation, for commands the
   * structured pipe cannot carry. */
  resume_source_id?: string | null;
  /** A worktree of the workspace's repository this session runs in; absent means the workspace itself. */
  checkout_path?: string | null;
  checkout_branch?: string | null;
  error: string | null;
}

export interface EndpointProfile {
  environment?: EnvironmentOverrides;
  id: string;
  name: string;
  provider: ProviderKind;
  endpoint_url: string | null;
  model: string | null;
  permission_mode: "native" | "interactive" | "trusted" | "plan" | "blocked";
  secret_ref: string | null;
  proxy_url?: string | null;
  /** Reasoning effort default, when the client supports one. */
  effort?: string | null;
  model_aliases?: Record<string, string>;
  /** A live reference to native client configuration, not copied configuration contents. */
  native_config?: { source_id: string; config_dir: string; config_env?: string | null } | null;
  created_at: string;
}

export interface FileEntry { name: string; path: string; kind: "file" | "directory" | "symlink" | "other"; size: number; }
export interface GitFile { index: string; worktree: string; path: string; original_path?: string; }
export interface GitStatus { branch: string | null; files: GitFile[]; ahead?: number; behind?: number; }
export interface GitDiff { diff: string; path: string | null; staged: boolean; binary: boolean; truncated: boolean; }
export interface TextFile { path: string; content: string; version: string; }
export interface NativeHistorySource { id:string; provider:AgentProviderKind; label:string; path:string; available:boolean; config_env?:string|null; }
export interface NativeHistoryItem { id:string; provider:AgentProviderKind; title:string; cwd:string; updated_at:string; imported_session_id:string|null; }
export interface ModelCatalog { models: Array<{id:string;name:string;efforts?:string[]}>; source_url: string; has_more: boolean; }
export type WorkspaceLayout = Record<string, unknown>;
export * from "./layout";
