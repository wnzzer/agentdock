import type { NativeModel } from "./chat-model";

/**
 * The models a session can switch to.
 *
 * A client lists the models it knows, named by its own guess (an endpoint ID
 * beginning `claude-fable-5` reads as "Fable 5"), and its official ones —
 * Default, Opus, Sonnet — which behind a gateway either do not exist or only
 * map back to the main model. So once the endpoint has said what it serves,
 * that is the list: the endpoint's models under the endpoint's names, with
 * what the client knows about the same ID (its thinking levels, say), and the
 * model in use if the endpoint does not list it. Without an endpoint catalog,
 * as for an official account, the client's list stands.
 */
export function mergeModels(client: readonly NativeModel[], endpoint: readonly NativeModel[], current?: string): NativeModel[] {
  if (!endpoint.length) return [...client];
  const known = new Map(client.map(entry => [entry.id, entry]));
  const merged = endpoint.map(entry => {
    const own = known.get(entry.id);
    return own ? { ...own, isDefault: undefined, name: entry.name !== entry.id ? entry.name : own.name } : entry;
  });
  if (current && !merged.some(entry => entry.id === current)) {
    const own = known.get(current);
    merged.unshift(own ? { ...own, isDefault: undefined } : { id: current, name: current });
  }
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
