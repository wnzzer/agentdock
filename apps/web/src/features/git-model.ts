import type { GitFile } from "@agentdock/protocol";

export const isStagedFile = (file: GitFile) => file.index !== " " && file.index !== "?";
export const isChangedFile = (file: GitFile) => file.worktree !== " " || file.index === "?";
export const pathsForGitFiles = (files: GitFile[]) => [...new Set(files.flatMap(file => file.original_path ? [file.path, file.original_path] : [file.path]))];
export interface DiffRow { text: string; before: string | number; after: string | number; kind: "hunk" | "meta" | "added" | "removed" | "context"; }

/** Line numbers are calculated from real unified diff hunk headers. */
export function parseUnifiedDiff(diff: string): DiffRow[] {
  let before = 0, after = 0, inHunk = false;
  return diff.split("\n").map(text => {
    const match = text.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (match) { before = Number(match[1]); after = Number(match[2]); inHunk = true; return { text, before: "", after: "", kind: "hunk" }; }
    if (!inHunk || text.startsWith("diff --git")) { if (text.startsWith("diff --git")) inHunk = false; return { text, before: "", after: "", kind: "meta" }; }
    if (text.startsWith("+")) return { text, before: "", after: after++, kind: "added" };
    if (text.startsWith("-")) return { text, before: before++, after: "", kind: "removed" };
    if (text.startsWith(" ")) return { text, before: before++, after: after++, kind: "context" };
    return { text, before: "", after: "", kind: "meta" };
  });
}
