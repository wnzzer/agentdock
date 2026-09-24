#!/usr/bin/env node
// Fuzzy file search for a workspace, backed by the Codex app-server's own index.
//
// Reads one JSON job from stdin: {cwd, query, limit, config_dir}
// Writes one JSON result to stdout: {files:[{path,name,kind,score,indices}],truncated}
//
// The index is local and needs no account: verified against an empty CODEX_HOME,
// where the search still returns results. So this runs with a config directory
// that holds no credentials and no history, and cannot read or alter the user's
// own Codex state — unlike the history bridge, which must use the real one.
import { withoutAgentDockSecrets } from "./native-spawn.mjs";
import { AppServerClient, NOT_STARTED } from "./app-server.mjs";

const MAX_LIMIT = 50;
// The app-server accepts `limit` and ignores it: asking for 3 still returned 26.
// Capping here is what keeps a broad query from becoming a huge response.
const DEADLINE_MS = 6000;
// A large root does not fail, it simply keeps working — /Users/kl was still going
// after 10s. A deadline turns that into a degraded answer instead of a hang.

function output(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}
function die(message) {
  output({ error: message });
  process.exit(0); // The caller reads a result; a non-zero exit would lose the reason.
}

const job = await new Promise((resolve, reject) => {
  let raw = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", chunk => { raw += chunk; if (raw.length > 64 * 1024) reject(new Error("Job too large")); });
  process.stdin.on("end", () => { try { resolve(JSON.parse(raw)); } catch { reject(new Error("Malformed job")); } });
}).catch(error => die(error.message));

const query = typeof job.query === "string" ? job.query.trim() : "";
// An empty query returns nothing from the index anyway; not spawning says so faster.
if (!query) { output({ files: [], truncated: false }); process.exit(0); }
const limit = Math.min(Number.isInteger(job.limit) && job.limit > 0 ? job.limit : 20, MAX_LIMIT);

const environment = withoutAgentDockSecrets(process.env);
if (job.config_dir) environment.CODEX_HOME = job.config_dir;

let settled = false;
const finish = value => {
  if (settled) return;
  settled = true;
  server.child.kill("SIGTERM");
  output(value);
  process.exit(0);
};
// A launch failure and an early exit each get their own answer: the first means
// Codex is missing, which the UI treats differently from a search that failed.
const server = new AppServerClient(process.env.AGENTDOCK_CODEX_BIN || "codex", ["app-server"], {
  cwd: job.cwd, env: environment,
  onClose: reason => finish({ error: reason === NOT_STARTED ? "Codex is not installed or could not start" : "Codex app-server exited before answering" }),
});
const timer = setTimeout(() => finish({ files: [], truncated: true, timed_out: true }), DEADLINE_MS);
timer.unref?.();
for (const signal of ["SIGTERM", "SIGINT"]) process.once(signal, () => { server.child.kill("SIGTERM"); process.exit(0); });

try {
  await server.initialize({ name: "agentdock", version: "0.1.0" });
  // Confirmed against codex 0.154.0: `roots` is required and must be an array;
  // `root` and `cwd` are both rejected with "missing field `roots`".
  const result = await server.request("fuzzyFileSearch", { query, roots: [job.cwd], limit });
  const rows = Array.isArray(result?.files) ? result.files : [];
  finish({
    files: rows.slice(0, limit).map(row => ({
      path: String(row.path ?? ""),
      name: String(row.file_name ?? ""),
      kind: row.match_type === "directory" ? "directory" : "file",
      score: Number.isFinite(row.score) ? row.score : 0,
      // Match positions, so the UI can show why a result matched rather than
      // re-deriving it and disagreeing with the index that ranked it.
      indices: Array.isArray(row.indices) ? row.indices.filter(Number.isInteger).slice(0, 256) : [],
    })).filter(row => row.path),
    truncated: rows.length > limit,
  });
} catch {
  finish({ error: "Codex could not complete the search" });
}
