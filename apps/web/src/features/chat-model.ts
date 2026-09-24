import type { EndpointProfile, Session } from '@agentdock/protocol';

/** A model exactly as the native client described it. `efforts` lists the
 * thinking depths that model supports; a model that states none supports none,
 * and no default ladder is substituted. */
/** `isDefault` marks the model a configuration resolves to when none is named,
 * which is what a not-yet-started session shows instead of guessing. */
export interface NativeModel { id: string; name: string; description?: string; efforts?: string[]; isDefault?: boolean }
export interface ApprovalQuestion { id: string; header?: string; question: string; options: Array<{ label: string; description?: string }>; isSecret?: boolean; isOther?: boolean; multiSelect?: boolean }
export type ChatEvent = ({ seq?: number } & (
  | { type: 'ready'; native_session_id?: string; commands?: string[] }
  | { type: 'settings'; model?: string; effort?: string; models?: NativeModel[]; permission_mode?: string; permission_modes?: string[] }
  | { type: 'message'; id: string; role: 'user' | 'assistant'; text: string; delta?: boolean; steered?: boolean }
  | { type: 'tool'; id: string; name: string; status: 'running' | 'completed' | 'failed'; text?: string; activity?: string }
  | { type: 'approval'; id: string; title: string; text: string; choices: string[]; questions?: ApprovalQuestion[] }
  | { type: 'approval_resolved'; id: string }
  | { type: 'turn'; id?: string; status: 'running' | 'completed' | 'failed' | 'interrupted' }
  | { type: 'usage'; input_tokens?: number; output_tokens?: number; context_tokens?: number; context_window?: number }
  | { type: 'configuration'; id: string; profile_name: string; text: string }
  | { type: 'error'; message: string }
  | { type: 'exit' }
  | { type: 'cleared' }
));
export interface ConversationSnapshot { mode?: 'structured' | 'pty'; running: boolean; events: ChatEvent[]; truncated?: boolean }
export type ChatItem =
  | { type: 'message'; id: string; role: 'user' | 'assistant'; text: string; steered?: boolean }
  | { type: 'tool'; id: string; name: string; status: 'running' | 'completed' | 'failed'; text: string; activity?: string }
  | { type: 'approval'; id: string; title: string; text: string; choices: string[]; questions: ApprovalQuestion[]; resolved: boolean }
  | { type: 'configuration'; id: string; profile_name: string; text: string }
  | { type: 'error'; id: string; text: string };

/** Newest-first truncation: the latest subagent step matters more than its first. */
const MAX_ACTIVITY = 4000;
function clipTail(value: string, limit: number) {
  return value.length <= limit ? value : `…\n${value.slice(value.length - limit)}`;
}

export function conversationView(events: readonly ChatEvent[], running?: boolean) {
  const items: ChatItem[] = [], indexes = new Map<string, number>(), seen = new Set<number>();
  let turn: 'idle' | 'running' | 'completed' | 'failed' | 'interrupted' = 'idle';
  let ready = false, exited = false, commands: string[] = [], announced = false, models: NativeModel[] = [], model: string | undefined, effort: string | undefined, permissionMode: string | undefined, permissionModes: string[] = [], usage: { input_tokens?: number; output_tokens?: number; context_tokens?: number; context_window?: number } = {};
  for (const event of events) {
    if (event.seq !== undefined) { if (seen.has(event.seq)) continue; seen.add(event.seq); }
    if (event.type === 'message') {
      const key = 'message:' + event.id, index = indexes.get(key), previous = index === undefined ? undefined : items[index];
      const steered = event.steered === true || (previous?.type === 'message' && previous.steered === true);
      const next: ChatItem = { type: 'message', id: event.id, role: event.role, text: event.delta && previous?.type === 'message' ? previous.text + event.text : event.text, ...(steered ? { steered: true } : {}) };
      if (index === undefined) { indexes.set(key, items.length); items.push(next); } else items[index] = next;
    } else if (event.type === 'tool') {
      const key = 'tool:' + event.id, index = indexes.get(key), previous = index === undefined ? undefined : items[index];
      // Subagent progress accumulates so the card reads as a log, while `text`
      // still carries only the tool's own input and result. Capped because a
      // long-running subagent would otherwise grow this without bound.
      const before = previous?.type === 'tool' ? previous : undefined;
      const activity = event.activity === undefined ? before?.activity
        : clipTail(before?.activity ? `${before.activity}\n${event.activity}` : event.activity, MAX_ACTIVITY);
      const next: ChatItem = { ...event, text: event.text ?? before?.text ?? '', ...(activity === undefined ? {} : { activity }) };
      if (index === undefined) { indexes.set(key, items.length); items.push(next); } else items[index] = next;
    } else if (event.type === 'approval') {
      const key = 'approval:' + event.id, index = indexes.get(key);
      const next: ChatItem = { ...event, questions: event.questions ?? [], resolved: false };
      if (index === undefined) { indexes.set(key, items.length); items.push(next); } else items[index] = next;
    } else if (event.type === 'approval_resolved') {
      const index = indexes.get('approval:' + event.id), previous = index === undefined ? undefined : items[index];
      if (previous?.type === 'approval') previous.resolved = true;
    } else if (event.type === 'turn') turn = event.status;
    else if (event.type === 'ready') { ready = true; exited = false; commands = event.commands ?? []; announced = true; }
    else if (event.type === 'settings') { if (event.models?.length) models = event.models; if (event.model) model = event.model; if (event.effort) effort = event.effort; if (event.permission_mode) permissionMode = event.permission_mode; if (event.permission_modes?.length) permissionModes = event.permission_modes; }
    else if (event.type === 'usage') { const { seq: _seq, type: _type, ...values } = event; usage = { ...usage, ...values }; }
    // A new endpoint means a new native client: its command list is whatever it
    // announces next, never the previous client's.
    else if (event.type === 'configuration') { for (const item of items) if (item.type === 'approval') item.resolved = true; indexes.clear(); items.push(event); ready = false; turn = 'idle'; commands = []; announced = false; models = []; model = undefined; effort = undefined; permissionMode = undefined; permissionModes = []; }
    // The client cleared its own context. Keeping the transcript would show a
    // conversation the client can no longer refer to, which is what made
    // /clear look like it had done nothing.
    else if (event.type === 'cleared') { items.length = 0; indexes.clear(); usage = {}; }
    else if (event.type === 'error') items.push({ type: 'error', id: 'error:' + (event.seq ?? items.length), text: event.message });
    else if (event.type === 'exit') { exited = true; ready = false; if (turn === 'running') turn = 'interrupted'; for (const item of items) if (item.type === 'approval') item.resolved = true; }
  }
  // Runtime state outranks stale display history after server restart. Old
  // approvals refer to lost native RPCs and cannot be answered as live ones.
  if (running === false) { ready = false; if (turn === 'running') turn = 'interrupted'; for (const item of items) if (item.type === 'approval') item.resolved = true; }
  return { items, turn, ready, exited, usage, commands, commandsAnnounced: announced, models, model, effort, permissionMode, permissionModes, awaitingApproval: items.some(item => item.type === 'approval' && !item.resolved) };
}

/** WS events are already ordered. A replayed sequence must not append deltas twice. */
export function appendChatEvent(events: readonly ChatEvent[], event: ChatEvent): ChatEvent[] {
  if (event.seq !== undefined && events.some(existing => existing.seq === event.seq)) return [...events];
  return [...events, event];
}
export function pruneChatEvents(events: readonly ChatEvent[], maxHistoryEvents = 2000, maxHistoryBytes = 8 * 1024 * 1024) {
  const controls = new Map<string, ChatEvent>(), approvals = new Map<string, ChatEvent>();
  const size = (event: ChatEvent) => new TextEncoder().encode(JSON.stringify(event)).byteLength;
  for (const event of events) {
    // The model list and the current selection can arrive in separate settings
    // events, so the last of each is kept rather than only the last overall.
    if (['ready', 'turn', 'exit', 'configuration', 'usage', 'settings', 'cleared'].includes(event.type)) {
      controls.set(event.type === 'settings' && event.models?.length ? 'settings:models' : event.type, event);
    }
    if (event.type === 'approval') approvals.set(event.id, event);
    if (event.type === 'approval_resolved') approvals.delete(event.id);
    if (event.type === 'configuration' || event.type === 'exit' || event.type === 'cleared') approvals.clear();
  }
  if (approvals.size > 32) throw Error('Too many unresolved native approvals. Reconnect the view before continuing.');
  const anchors = new Set([...controls.values(), ...approvals.values()]);
  let anchorBytes = 0;
  for (const event of anchors) { const bytes = size(event); if (bytes > 192 * 1024) throw Error('A native control event exceeds the supported display limit.'); anchorBytes += bytes; }
  const keep = new Set(anchors); let historyBytes = 0, historyCount = 0;
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]; if (anchors.has(event)) continue;
    const bytes = size(event);
    if (historyCount >= maxHistoryEvents || historyBytes + bytes > maxHistoryBytes) break;
    keep.add(event); historyBytes += bytes; historyCount++;
  }
  // Filtering the original ordered stream preserves ready/exit/configuration
  // boundaries; protected old approvals must never be appended out of order.
  return { events: events.filter(event => keep.has(event)), truncated: keep.size < events.length, historyBytes, anchorBytes };
}
export function isChatEvent(value: unknown): value is ChatEvent {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>, text = (key: string) => typeof v[key] === 'string';
  if (v.seq !== undefined && (typeof v.seq !== 'number' || !Number.isSafeInteger(v.seq) || v.seq < 0)) return false;
  switch (v.type) {
    case 'ready': return (v.native_session_id === undefined || text('native_session_id'))
      && (v.commands === undefined || Array.isArray(v.commands) && v.commands.length <= 400 && v.commands.every(name => typeof name === 'string' && /^[A-Za-z0-9][\w:-]{0,63}$/.test(name)));
    case 'settings': return (v.model === undefined || text('model')) && (v.effort === undefined || text('effort'))
      && (v.permission_mode === undefined || text('permission_mode'))
      && (v.permission_modes === undefined || Array.isArray(v.permission_modes) && v.permission_modes.length <= 8 && v.permission_modes.every(mode => typeof mode === 'string' && /^[a-z_]{1,24}$/.test(mode)))
      && (v.models === undefined || Array.isArray(v.models) && v.models.length <= 64 && v.models.every(entry => {
        if (!entry || typeof entry !== 'object') return false;
        const m = entry as Record<string, unknown>;
        return typeof m.id === 'string' && !!m.id && typeof m.name === 'string'
          && (m.description === undefined || typeof m.description === 'string')
          && (m.efforts === undefined || Array.isArray(m.efforts) && m.efforts.every(level => typeof level === 'string'));
      }));
    case 'exit': case 'cleared': return true;
    case 'message': return text('id') && text('text') && ['user', 'assistant'].includes(String(v.role)) && (v.delta === undefined || typeof v.delta === 'boolean');
    case 'tool': return text('id') && text('name') && ['running', 'completed', 'failed'].includes(String(v.status)) && (v.text === undefined || text('text')) && (v.activity === undefined || text('activity'));
    case 'approval': return text('id') && text('title') && text('text') && Array.isArray(v.choices) && v.choices.every(choice => typeof choice === 'string') && (v.questions === undefined || Array.isArray(v.questions) && v.questions.every(question => {
      if (!question || typeof question !== 'object') return false;
      const q = question as Record<string, unknown>;
      return typeof q.id === 'string' && typeof q.question === 'string' && ['isSecret', 'isOther', 'multiSelect'].every(key => q[key] === undefined || typeof q[key] === 'boolean') && Array.isArray(q.options) && q.options.every(option => !!option && typeof option === 'object' && typeof (option as Record<string, unknown>).label === 'string');
    }));
    case 'approval_resolved': return text('id');
    case 'turn': return ['running', 'completed', 'failed', 'interrupted'].includes(String(v.status));
    case 'usage': return ['input_tokens', 'output_tokens', 'context_tokens', 'context_window'].every(key => v[key] === undefined || typeof v[key] === 'number' && Number.isFinite(v[key]) && Number(v[key]) >= 0);
    case 'error': return text('message');
    case 'configuration': return text('id') && text('profile_name') && text('text');
    default: return false;
  }
}
export function parseConversationSnapshot(value: unknown): ConversationSnapshot {
  if (!value || typeof value !== 'object') throw Error('Invalid conversation response.');
  const v = value as Record<string, unknown>;
  if (typeof v.running !== 'boolean' || !Array.isArray(v.events) || !v.events.every(isChatEvent) || (v.mode !== undefined && v.mode !== 'structured' && v.mode !== 'pty')) throw Error('Invalid conversation response.');
  return { mode: v.mode as ConversationSnapshot['mode'], running: v.running, events: v.events, truncated: v.truncated === true };
}

export function approvalPayloadAnswers(questions: readonly ApprovalQuestion[], selections: Record<string, string[]> = {}, others: Record<string, string> = {}): Record<string, string[]> {
  return Object.fromEntries(questions.flatMap(question => {
    const raw = Object.hasOwn(selections, question.id) ? selections[question.id] : [];
    let values = raw.filter(value => typeof value === 'string' && value.trim() && (question.isSecret || question.isOther || !question.options.length || question.options.some(option => option.label === value)));
    const other = question.isOther && Object.hasOwn(others, question.id) ? others[question.id] : undefined;
    if (other?.trim()) values = question.multiSelect && !question.isSecret ? [...values, other] : [other];
    values = [...new Set(values)];
    if (!question.multiSelect || question.isSecret) values = values.slice(0, 1);
    return values.length ? [[question.id, values]] : [];
  }));
}

/**
 * Whether the client's own /clear can be offered right now.
 *
 * This is a pure function because the condition is easy to get subtly wrong:
 * it once included "a message could be sent", which requires typed text, so
 * the action disappeared whenever the composer was empty — exactly when it is
 * wanted. Clearing needs a live, idle client that advertised the command, and
 * nothing about what is in the composer.
 */
export function canClearContext(state: {
  preview: boolean;
  running: boolean;
  ready: boolean;
  busy: boolean;
  connected: boolean;
  commands: readonly string[];
}): boolean {
  return !state.preview && state.running && state.ready && !state.busy && state.connected && state.commands.includes('clear');
}

export interface ChatDraft {
  text: string;
  pending?: {
    id: string;
    /** What went on the wire: trimmed, and with any attachment header. */
    content: string;
    /** What the composer held when it was sent. The wire content is not the
     * same string — it is trimmed and may carry an attachment header — so
     * comparing against it left the composer holding a message already sent. */
    text: string;
    state: 'sending' | 'unknown';
    profileId?: string | null;
    configurationRevision?: number;
  };
  notice?: string;
}
export function createChatDraftStore() {
  const drafts = new Map<string, ChatDraft>();
  return { get(id: string): ChatDraft { let draft = drafts.get(id); if (!draft) { draft = { text: '' }; drafts.set(id, draft); } return draft; } };
}
export const chatDrafts = createChatDraftStore();
export function acknowledgeDraft(draft: ChatDraft, events: readonly ChatEvent[]): boolean {
  const pending = draft.pending;
  if (!pending || !events.some(event => event.type === 'message' && event.role === 'user' && event.id === pending.id)) return false;
  // Only the message that was sent is cleared; anything typed since stays.
  if (draft.text === pending.text) draft.text = '';
  draft.pending = undefined; draft.notice = undefined; return true;
}
export function acknowledgeReceipt(draft: ChatDraft, receipt: unknown): boolean {
  const pending=draft.pending;
  if(!pending || !receipt || typeof receipt!=='object' || (receipt as {accepted?:unknown}).accepted!==true || (receipt as {id?:unknown}).id!==pending.id)return false;
  if(draft.text===pending.text)draft.text='';
  draft.pending=undefined;draft.notice=undefined;return true;
}
export function pendingMessageRetry(draft: ChatDraft, session: Pick<Session, 'endpoint_profile_id' | 'configuration_revision'>) {
  const pending = draft.pending;
  if (!pending || pending.state !== 'unknown') throw Error('There is no uncertain message to retry.');
  if ((pending.profileId ?? null) !== (session.endpoint_profile_id ?? null) || (pending.configurationRevision??0)!==(session.configuration_revision??0)) throw Error('The endpoint changed while delivery was uncertain. This request will not be retried with another account.');
  return { id: pending.id, content: pending.content };
}
export function sessionConfigurationPayload(
  session: Session, profiles: EndpointProfile[], profileId: string | null, confirmed: boolean,
  context?: { mode?: string; ready: boolean; busy: boolean; awaitingApproval: boolean },
  /** Session-only overrides; omitted fields keep whatever the profile provides. */
  overrides?: { model?: string; effort?: string },
) {
  if (!confirmed) throw Error('Confirm the endpoint change first.');
  if (!['stopped', 'failed'].includes(session.status) && !(context?.mode === 'structured' && context.ready && !context.busy && !context.awaitingApproval)) throw Error('Wait for the current turn and approvals before changing endpoints.');
  if (profileId !== null && !profiles.some(profile => profile.id === profileId && profile.provider === session.provider)) throw Error('Choose an endpoint for the same client.');
  const model = overrides?.model?.trim();
  const effort = overrides?.effort?.trim();
  return {
    endpoint_profile_id: profileId,
    ...(model ? { model } : {}),
    ...(effort ? { effort } : {}),
    confirmed: true,
  };
}

export type MarkdownInline = { type: 'text' | 'strong' | 'em' | 'code'; text: string } | { type: 'link'; text: string; href: string } | { type: 'image'; alt: string; src: string } | { type: 'file'; text: string; path: string; line?: number; code: boolean };
export type MarkdownBlock = { type: 'paragraph' | 'heading' | 'quote'; text: string; level?: number } | { type: 'code'; text: string; language: string } | { type: 'list'; ordered: boolean; items: string[] } | { type: 'table'; header: string[]; align: Array<'left' | 'center' | 'right' | undefined>; rows: string[][] };
/**
 * No HTML or executable URL schemes. The renderer uses Vue text nodes.
 *
 * Images are parsed but only ever loaded from the workspace, through this
 * server; whoever renders them decides that (see workspaceImagePath). A remote
 * image in text a model wrote would otherwise make the browser fetch any URL
 * it named.
 */
export function safeWebUrl(value: string): string | undefined {
  try { const url = new URL(value); return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined; } catch { return undefined; }
}
export function markdownInline(text: string): MarkdownInline[] {
  const result: MarkdownInline[] = [], pattern = /(`[^`\n]+`|!\[[^\]\n]*\]\([^\s)]+\)|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\([^\s)]+\))/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0, token = match[0];
    if (start > cursor) result.push(...linkProse(text.slice(cursor, start)));
    if (token.startsWith('`')) { const code = token.slice(1, -1), reference = fileReference(code); result.push(reference ? { type: 'file', text: code, path: reference.path, line: reference.line, code: true } : { type: 'code', text: code }); }
    else if (token.startsWith('**')) result.push({ type: 'strong', text: token.slice(2, -2) });
    else if (token.startsWith('*')) result.push({ type: 'em', text: token.slice(1, -1) });
    else if (token.startsWith('![')) {
      // Only a path inside the workspace becomes an image. A remote one is
      // offered as a link to follow, never fetched on its own.
      const bracket = token.indexOf(']('), alt = token.slice(2, bracket), src = token.slice(bracket + 2, -1), href = safeWebUrl(src);
      result.push(isLocalImageReference(src) ? { type: 'image', alt, src } : href ? { type: 'link', text: alt || href, href } : { type: 'text', text: token });
    }
    else {
      const bracket = token.indexOf(']('), target = token.slice(bracket + 2, -1), href = safeWebUrl(target), reference = href ? undefined : fileReference(target);
      result.push(href ? { type: 'link', text: token.slice(1, bracket), href } : reference ? { type: 'file', text: token.slice(1, bracket), path: reference.path, line: reference.line, code: false } : { type: 'text', text: token });
    }
    cursor = start + token.length;
  }
  if (cursor < text.length) result.push(...linkProse(text.slice(cursor)));
  return result;
}
/** Cells of a `| a | b |` row. A `|` inside backticks or escaped as `\\|` is text. */
function tableCells(line: string): string[] {
  const cells: string[] = []; let cell = '', code = false;
  const body = line.trim().replace(/^\|/, '').replace(/(?<!\\)\|$/, '');
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === '\\' && body[i + 1] === '|') { cell += '|'; i++; continue; }
    if (c === '`') code = !code;
    if (c === '|' && !code) { cells.push(cell.trim()); cell = ''; continue; }
    cell += c;
  }
  cells.push(cell.trim());
  return cells;
}
const TABLE_DIVIDER = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
export function markdownBlocks(text: string): MarkdownBlock[] {
  const lines = text.replace(/\r\n/g, '\n').split('\n'), blocks: MarkdownBlock[] = [];
  for (let i = 0; i < lines.length;) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    const fence = line.match(/^\s*```([^`]*)$/);
    if (fence) { const body: string[] = []; i++; while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) body.push(lines[i++]); if (i < lines.length) i++; blocks.push({ type: 'code', language: fence[1].trim(), text: body.join('\n') }); continue; }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) { blocks.push({ type: 'heading', level: heading[1].length, text: heading[2] }); i++; continue; }
    if (/^>\s?/.test(line)) { blocks.push({ type: 'quote', text: line.replace(/^>\s?/, '') }); i++; continue; }
    // A GFM table: a row, a divider of dashes, then rows until one without a pipe.
    if (line.includes('|') && i + 1 < lines.length && TABLE_DIVIDER.test(lines[i + 1])) {
      const header = tableCells(line), divider = tableCells(lines[i + 1]);
      if (divider.length === header.length) {
        const align = divider.map(cell => cell.startsWith(':') && cell.endsWith(':') ? 'center' as const : cell.endsWith(':') ? 'right' as const : cell.startsWith(':') ? 'left' as const : undefined);
        const rows: string[][] = []; i += 2;
        while (i < lines.length && lines[i].trim() && lines[i].includes('|')) { const cells = tableCells(lines[i++]); rows.push(header.map((_, index) => cells[index] ?? '')); }
        blocks.push({ type: 'table', header, align, rows }); continue;
      }
    }
    const list = line.match(/^\s*(?:([-*+])|(\d+)\.)\s+(.*)$/);
    if (list) {
      const ordered = !!list[2], items: string[] = [];
      while (i < lines.length) { const item = lines[i].match(/^\s*(?:([-*+])|(\d+)\.)\s+(.*)$/); if (!item || !!item[2] !== ordered) break; items.push(item[3]); i++; }
      blocks.push({ type: 'list', ordered, items }); continue;
    }
    const body = [line]; i++;
    while (i < lines.length && lines[i].trim() && !/^(?:\s*```|#{1,6}\s|>\s?|\s*(?:[-*+]|\d+\.)\s)/.test(lines[i]) && !(lines[i].includes('|') && TABLE_DIVIDER.test(lines[i + 1] ?? ''))) body.push(lines[i++]);
    blocks.push({ type: 'paragraph', text: body.join('\n') });
  }
  return blocks;
}

const IMAGE_EXTENSION = /\.(png|jpe?g|gif|webp|svg|avif|bmp|ico)$/i;
/**
 * A workspace-relative path for an image reference, or undefined when it is
 * not one: a URL with a scheme, an absolute path, or one that climbs out of
 * the workspace. `base` is the directory of the file it appears in, so
 * `![](img/a.png)` in docs/readme.md means docs/img/a.png, as on GitHub.
 */
export function workspaceImagePath(src: string, base = ''): string | undefined {
  let value = src.trim();
  try { value = decodeURIComponent(value); } catch { return undefined; }
  value = value.split(/[?#]/)[0];
  if (!value || /^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith('//') || !IMAGE_EXTENSION.test(value)) return undefined;
  const parts: string[] = [];
  for (const part of (value.startsWith('/') ? value.slice(1) : `${base ? base + '/' : ''}${value}`).split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') { if (!parts.length) return undefined; parts.pop(); continue; }
    parts.push(part);
  }
  return parts.length ? parts.join('/') : undefined;
}
/** A reference to an image on disk rather than on the web. Where it points is
 * settled when rendering, against the file it appears in. */
export function isLocalImageReference(src: string): boolean {
  const value = src.trim().split(/[?#]/)[0];
  return !!value && !/^[a-z][a-z0-9+.-]*:/i.test(value) && !value.startsWith('//') && IMAGE_EXTENSION.test(value);
}
/** Image files a message says it attached, from composeMessage's header. */
export function attachedImagePaths(text: string): string[] {
  const match = text.match(/^Attached files? in this workspace:\n((?:- .+\n?)+)/);
  if (!match) return [];
  return match[1].split('\n').map(line => line.replace(/^- /, '').trim()).filter(path => IMAGE_EXTENSION.test(path) && workspaceImagePath(path) === path);
}

/** A place in a file a message points at. */
export interface FileReference { path: string; line?: number }
const FILE_PATTERN = /^(?:file:\/\/)?((?:\/?[A-Za-z]:[\\/]|\/|\.{1,2}[\\/])?(?:[\w.@+-]+[\\/])*[\w@+-][\w.@+-]*\.[A-Za-z][A-Za-z0-9]{0,9})(?:(?::|#L)(\d{1,7})(?::\d{1,5}|-L?\d{1,7})?)?$/;
/**
 * A file path, as agents write them: `src/main.rs`, `main.rs:42`,
 * `src/main.rs:42:7`, `main.rs#L42`, an absolute path, or a `file://` URL.
 * A bare name counts only with an extension that starts with a letter, so a
 * version (`1.2.3`) or a number is not taken for a file. Agents on a Windows
 * host write `C:\repo\a.ts` or `src\a.ts`; those come back with `/` separators,
 * which the server accepts there too, so one form flows through the UI.
 */
export function fileReference(value: string): FileReference | undefined {
  const match = value.trim().match(FILE_PATTERN);
  if (!match) return undefined;
  let path = match[1].replace(/\\/g, '/').replace(/^\/(?=[A-Za-z]:\/)/, '');
  try { path = decodeURIComponent(path); } catch { return undefined; }
  if (path.includes('//') || /^\d/.test(path.split('/').pop() ?? '')) return undefined;
  return match[2] ? { path, line: Number(match[2]) } : { path };
}
/** Plain prose is scanned only for paths with a directory in them, and URLs;
 * a bare "main.rs" in a sentence stays a word. */
const PROSE_REFERENCES = /(https?:\/\/[^\s<>()"'`]+[^\s<>()"'`.,;:!?。，；：！？)])|((?:\/|\.{1,2}\/)?(?:[\w.@+-]+\/)+[\w@+-][\w.@+-]*\.[A-Za-z][A-Za-z0-9]{0,9}(?::\d{1,7}(?::\d{1,5})?|#L\d{1,7})?)/g;
export function linkProse(text: string): MarkdownInline[] {
  const parts: MarkdownInline[] = [];
  let cursor = 0;
  for (const match of text.matchAll(PROSE_REFERENCES)) {
    const start = match.index ?? 0, token = match[0];
    const reference = match[2] ? fileReference(token) : undefined;
    const href = match[1] ? safeWebUrl(token) : undefined;
    if (!reference && !href) continue;
    if (start > cursor) parts.push({ type: 'text', text: text.slice(cursor, start) });
    parts.push(reference ? { type: 'file', text: token, path: reference.path, line: reference.line, code: false } : { type: 'link', text: token, href: href! });
    cursor = start + token.length;
  }
  if (cursor < text.length) parts.push({ type: 'text', text: text.slice(cursor) });
  return parts.length ? parts : [{ type: 'text', text }];
}
