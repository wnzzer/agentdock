import type { AgentProviderKind, ModelCatalog } from '@agentdock/protocol';

/** The values accepted by both native clients' configuration surfaces. */
export const REASONING_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'] as const;
export type ReasoningEffort = typeof REASONING_EFFORTS[number];

/**
 * Claude Code exposes --effort as a client option. Codex is model-specific and
 * must use the levels advertised by its model/list response; an empty catalog
 * deliberately produces no guessed choices.
 */
export function modelEfforts(
  provider: AgentProviderKind,
  model: string | undefined,
  catalog: ModelCatalog | undefined,
  current?: string | null,
): string[] {
  const advertised = model ? catalog?.models.find(entry => entry.id === model)?.efforts ?? [] : [];
  const values = provider === 'claude_code' && !advertised.length
    ? [...REASONING_EFFORTS]
    : advertised.filter(value => (REASONING_EFFORTS as readonly string[]).includes(value));
  if (current && (REASONING_EFFORTS as readonly string[]).includes(current) && !values.includes(current)) values.push(current);
  return [...new Set(values)];
}

export function effortLabel(value: string): string {
  return value === 'xhigh' ? 'X-high' : value[0].toUpperCase() + value.slice(1);
}
