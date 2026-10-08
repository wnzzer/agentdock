import type { NativeModel } from "./chat-model";

/**
 * The models a session can switch to: what its client reports, and what its
 * endpoint serves.
 *
 * A client lists the models it knows and the one it was started with, named
 * by its own guess (an endpoint ID beginning `claude-fable-5` reads as "Fable
 * 5"). An endpoint behind a gateway serves many more, under IDs nobody would
 * type, with names people do. So the endpoint's models are added after the
 * client's, and where both list an ID, the endpoint's name is the one shown.
 */
export function mergeModels(client: readonly NativeModel[], endpoint: readonly NativeModel[]): NativeModel[] {
  const named = new Map(endpoint.map(entry => [entry.id, entry.name]));
  const merged = client.map(entry => named.has(entry.id) && named.get(entry.id) !== entry.id ? { ...entry, name: named.get(entry.id)! } : entry);
  const listed = new Set(merged.map(entry => entry.id));
  for (const entry of endpoint) if (!listed.has(entry.id)) { merged.push(entry); listed.add(entry.id); }
  return merged;
}

/**
 * What a model typed by hand means: an ID as given, or else the ID of the one
 * endpoint model with that name. Sending the name would reach the endpoint as
 * a model it does not have.
 */
export function resolveModel(input: string, endpoint: readonly NativeModel[]): string {
  const typed = input.trim();
  if (!typed || endpoint.some(entry => entry.id === typed)) return typed;
  const named = endpoint.filter(entry => entry.name.toLocaleLowerCase() === typed.toLocaleLowerCase());
  return named.length === 1 ? named[0].id : typed;
}

/** The models whose name or ID contains `query`, all of them for an empty one. */
export function filterModels(models: readonly NativeModel[], query: string): NativeModel[] {
  const term = query.trim().toLocaleLowerCase();
  return term ? models.filter(entry => `${entry.name} ${entry.id}`.toLocaleLowerCase().includes(term)) : [...models];
}
