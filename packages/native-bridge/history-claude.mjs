import { LIMIT, normalizeSessions } from "./history-common.mjs";

export const claudeSession=row=>({id:row.sessionId,cwd:row.cwd,time:row.lastModified});
/** @param {string} cwd @param {string} configDir @param {string} configEnv */
export async function claudeHistory(cwd,configDir,configEnv) {
  process.env[configEnv]=configDir;
  // listSessions is a metadata-only SDK helper; no query() or tool loop is started.
  const { listSessions }=await import("@anthropic-ai/claude-agent-sdk");
  const rows=await listSessions({dir:cwd,includeWorktrees:false,limit:LIMIT+1});
  return {items:await normalizeSessions(rows.slice(0,LIMIT),"claude_code",cwd,claudeSession),truncated:rows.length>LIMIT};
}
