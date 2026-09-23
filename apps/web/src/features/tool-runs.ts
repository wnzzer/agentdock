import type { ChatItem } from './chat-model';

type ToolItem = Extract<ChatItem, { type: 'tool' }>;
/** A stretch of tool calls the agent made one after another, shown as one card. */
export interface ToolRun { type: 'tool_run'; id: string; tools: ToolItem[] }
export type DisplayItem = Exclude<ChatItem, { type: 'tool' }> | ToolRun;

/**
 * Consecutive tool calls, folded into runs.
 *
 * Five `Bash` cards in a row said the same thing five times and pushed the
 * answer off the screen. A run is one card that says how many calls it holds
 * and opens to each of them. A lone call is still a run of one, so it keeps its
 * card and its place; a failed call closes the run it ends so it is never
 * folded away behind a summary that reads as fine.
 */
export function toolRuns(items: readonly ChatItem[]): DisplayItem[] {
  const out: DisplayItem[] = [];
  let run: ToolRun | undefined;
  for (const item of items) {
    if (item.type !== 'tool') { run = undefined; out.push(item); continue; }
    if (item.status === 'failed') { run = undefined; out.push({ type: 'tool_run', id: item.id, tools: [item] }); continue; }
    if (!run) { run = { type: 'tool_run', id: item.id, tools: [] }; out.push(run); }
    run.tools.push(item);
  }
  return out;
}

/** "Bash ×3 · Read ×2", in the order the tools were first used. */
export function toolRunNames(run: ToolRun): string {
  const counts = new Map<string, number>();
  for (const tool of run.tools) counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
  return [...counts].map(([name, count]) => count > 1 ? `${name} ×${count}` : name).join(' · ');
}

export function toolRunStatus(run: ToolRun): 'running' | 'failed' | 'completed' {
  return run.tools.some(tool => tool.status === 'running') ? 'running' : run.tools.some(tool => tool.status === 'failed') ? 'failed' : 'completed';
}
