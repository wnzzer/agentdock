/** The usage report as the server sends it, and the figures the page derives from it. */
export type UsageProvider = "claude_code" | "codex";
export interface UsageBucket {
  input: number;
  cache_read: number;
  cache_write_5m: number;
  cache_write_1h: number;
  output: number;
  reasoning: number;
  total: number;
  cost: number;
  unpriced: number;
  calls: number;
}
export interface Price { input: number; output: number; cache_read: number }
export interface UsageModel extends UsageBucket { model: string; provider: UsageProvider; price: Price | null; custom_price: boolean }
export interface UsageConversation extends UsageBucket {
  conversation: string;
  provider: UsageProvider;
  session_id: string | null;
  title: string | null;
  cwd: string;
  models: string[];
  first_at: number;
  last_at: number;
}
export interface UsageReport {
  from: number;
  to: number;
  totals: UsageBucket;
  previous: UsageBucket;
  days: Array<UsageBucket & { date: string }>;
  models: UsageModel[];
  providers: UsageModel[];
  heatmap: number[][];
  conversations: UsageConversation[];
  conversation_count: number;
  directories: Array<[string, number]>;
}
export interface Allowance {
  account_id: string;
  account: string;
  provider: UsageProvider;
  window: "primary" | "secondary";
  window_minutes: number;
  used_percent: number;
  window_start: number;
  resets_at: number;
  used_tokens: number;
  used_cost: number;
  estimated_total_tokens: number | null;
  estimated_remaining_tokens: number | null;
  estimated_total_cost: number | null;
  estimated_remaining_cost: number | null;
  low_confidence: boolean;
}

/** 1.2K, 34.5M, 1.1B: tokens at a glance. */
export function formatTokens(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  const units = [[1e9, "B"], [1e6, "M"], [1e3, "K"]] as const;
  for (const [size, unit] of units) if (value >= size) { const scaled = value / size; return `${scaled >= 100 ? Math.round(scaled) : scaled.toFixed(1)}${unit}`; }
  return String(Math.round(value));
}

export function formatCost(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "$0";
  return value >= 100 ? `$${Math.round(value).toLocaleString("en-US")}` : value >= 1 ? `$${value.toFixed(2)}` : `$${value.toFixed(3)}`;
}

/** Change against the previous period, or nothing when there was no previous. */
export function trend(current: number, previous: number): number | undefined {
  if (!(previous > 0)) return undefined;
  return (current - previous) / previous * 100;
}

/** Input the cache served, of all input: cache reads over reads, writes and uncached input. */
export function cacheHitRate(bucket: Pick<UsageBucket, "input" | "cache_read" | "cache_write_5m" | "cache_write_1h">): number {
  const input = bucket.input + bucket.cache_read + bucket.cache_write_5m + bucket.cache_write_1h;
  return input > 0 ? bucket.cache_read / input * 100 : 0;
}

/** What cache reads saved against paying full input price for them, where the price is known. */
export function cacheSavings(models: readonly UsageModel[]): number {
  return models.reduce((sum, row) => sum + (row.price ? row.cache_read * (row.price.input - row.price.cache_read) / 1e6 : 0), 0);
}

/** The local start of today, and of the day N days back, as Unix seconds. */
export function rangeStart(days: number, now = new Date()): number {
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate() - (days - 1));
  return Math.floor(start.getTime() / 1000);
}
