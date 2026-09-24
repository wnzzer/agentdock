import type { EndpointProfile, ProviderKind } from "@agentdock/protocol";

export const PROFILE_CHOICE_REQUIRED = "__choose_profile__";
const ISOLATED_PROFILE = "__isolated_profile__";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
type PreferenceStorage = Pick<Storage, "getItem" | "setItem">;
interface RememberedChoice { value: string; unsaved: boolean }
const storageMemory = new WeakMap<PreferenceStorage, Map<ProviderKind, RememberedChoice>>();
const unavailableStorageMemory = new Map<ProviderKind, RememberedChoice>();

function browserStorage(): PreferenceStorage | undefined {
  try { return typeof window === "undefined" ? undefined : window.localStorage; } catch { return undefined; }
}

function memory(storage: PreferenceStorage | undefined): Map<ProviderKind, RememberedChoice> {
  if (!storage) return unavailableStorageMemory;
  let choices = storageMemory.get(storage);
  if (!choices) { choices = new Map(); storageMemory.set(storage, choices); }
  return choices;
}

function key(provider: ProviderKind): string { return `agentdock.profile-selection.v1:${provider}`; }
function decode(value: string | null | undefined): string | undefined {
  if (value === ISOLATED_PROFILE) return "";
  return typeof value === "string" && UUID.test(value) ? value : undefined;
}

function rememberedSelection(provider: ProviderKind, storage: PreferenceStorage | undefined): string | undefined {
  const choices = memory(storage), cached = choices.get(provider);
  // A failed write must not be replaced by an older value still in localStorage.
  if (!storage || cached?.unsaved) return decode(cached?.value);
  try {
    const value = storage.getItem(key(provider)), selection = decode(value);
    if (selection === undefined) choices.delete(provider);
    else choices.set(provider, { value: value!, unsaved: false });
    return selection;
  } catch { return decode(cached?.value); }
}

export function isProfileSelectionValid(provider: ProviderKind, selection: string, profiles: EndpointProfile[]): boolean {
  if (selection === "") return true;
  return UUID.test(selection) && profiles.some(profile => profile.id === selection && profile.provider === provider);
}

/**
 * The default set in Preferences wins; then the last choice made here; then a
 * sole host profile. Multiple accounts with neither require an explicit choice.
 */
export function preferredProfileSelection(provider: ProviderKind, profiles: EndpointProfile[], storage?: PreferenceStorage, preferred?: string | null): string {
  if (preferred && isProfileSelectionValid(provider, preferred, profiles)) return preferred;
  const remembered = rememberedSelection(provider, storage ?? browserStorage());
  if (remembered !== undefined && isProfileSelectionValid(provider, remembered, profiles)) return remembered;
  const nativeIds = new Set(profiles.filter(profile => profile.provider === provider && !!profile.native_config && UUID.test(profile.id)).map(profile => profile.id));
  if (nativeIds.size === 1) return nativeIds.values().next().value!;
  return nativeIds.size > 1 ? PROFILE_CHOICE_REQUIRED : "";
}

/** Only an opaque profile UUID or the explicit isolated choice is persisted. Never profile contents. */
export function rememberProfileSelection(provider: ProviderKind, selection: string, storage?: PreferenceStorage): void {
  if (selection !== "" && !UUID.test(selection)) return;
  const target = storage ?? browserStorage();
  const remembered = { value: selection === "" ? ISOLATED_PROFILE : selection, unsaved: true };
  memory(target).set(provider, remembered);
  try { if (target) { target.setItem(key(provider), remembered.value); remembered.unsaved = false; } }
  catch { /* Keep the explicit choice in memory when browser storage is unavailable. */ }
}
