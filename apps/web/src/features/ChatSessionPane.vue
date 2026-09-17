<script setup lang="ts">
import { computed, h, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import type { EndpointProfile, Session } from '@agentdock/protocol';
import { ApiConnectionError, errorMessage, json, providerLabel, request } from './api';
import { backendCapabilities } from './backend-capabilities';
import { sessionConnections } from './session-connection';
import { createSessionStream, type SessionStreamState } from './session-stream';
import { acknowledgeDraft, appendChatEvent, approvalPayloadAnswers, canClearContext, chatDrafts, createChatDraftStore, conversationView, isChatEvent, parseConversationSnapshot, pendingMessageRetry, pruneChatEvents, sessionConfigurationPayload, type ChatDraft, type ChatEvent, type ChatItem, type ConversationSnapshot, type NativeModel } from './chat-model';
import ProviderIcon from './ProviderIcon.vue';
import Icon from './Icon.vue';
import ChipMenu from './ChipMenu.vue';
import ContextRing from './ContextRing.vue';
import { effortLabel, modelEfforts } from './reasoning-effort';
import { attachmentError, canSendMessage, composeMessage, formatBytes, MAX_ATTACHMENTS_PER_MESSAGE, type Attachment } from './attachment-state';
import { applyCommand, matchCommands, moveHighlight, slashQuery, unsupportedCommand } from './slash-commands';
import { MarkdownContent } from './MarkdownContent';
import { mentionQuery, applyMention } from './file-mentions';
import { createFileSearch } from './file-search';
import type { SearchHit } from './file-search';
import { useI18n } from '../i18n';
import { acknowledgeReceipt } from './chat-model';
import { useSessionMenuPosition } from './session-menu-position';
import { canReopenInTerminal, terminalReopen } from './terminal-reopen';
import { randomId } from './random-id';

const props = defineProps<{ paneId?: string; session: Session; profiles: EndpointProfile[]; previewSnapshot?: ConversationSnapshot; consumeOpenIntent?: boolean }>();
const emit = defineEmits<{ changed: []; environment: [id: string]; renameRequest: [id: string]; legacy: []; profiles: []; openSession: [session: Session] }>();
const { t } = useI18n();
const isPreview = computed(() => props.previewSnapshot !== undefined);
const capabilities = backendCapabilities as typeof backendCapabilities & { structuredChat?: boolean; sessionConfiguration?: boolean; sessionTerminalEscape?: boolean };
const supported = computed(() => isPreview.value || capabilities.structuredChat === true);
const mode = ref<'structured' | 'pty' | undefined>(props.previewSnapshot?.mode), running = ref(props.previewSnapshot?.running ?? false), truncated = ref(!!props.previewSnapshot?.truncated), events = ref<ChatEvent[]>(props.previewSnapshot?.events ?? []);
const legacyAvailable = computed(() => !isPreview.value && (mode.value === 'pty' || mode.value === undefined && (props.session as Session & { interaction_mode?: string }).interaction_mode !== 'structured'));
// An escape hatch is only meaningful while structured mode is actually the
// surface in use, and only once the client has a conversation to reopen.
const terminalReopenAvailable = computed(() => !isPreview.value && mode.value === 'structured' && canReopenInTerminal(props.session, capabilities));
const mounted = ref(false), loading = ref(false), actionBusy = ref(false), error = ref(''), notice = ref('');
const streamState = ref<SessionStreamState>(isPreview.value ? 'connected' : 'disconnected'), streamNotice = ref('');
const viewport = ref<HTMLElement>(), followBottom = ref(true), endpointConfirm = ref(false), selectedProfile = ref(props.session.endpoint_profile_id ?? '');
const confirmEnd = ref(false);
const sessionMenu = ref<HTMLDetailsElement>();
const composerInput = ref<HTMLTextAreaElement>();
const composing = ref(false);
const { tabStyle: tabMenuStyle, panelStyle: tabMenuPanelStyle, positionMenu } = useSessionMenuPosition(() => props.paneId, sessionMenu);
const approvalBusy = ref(''), approvalAnswers = reactive<Record<string, Record<string, string[]>>>(Object.create(null)), approvalOther = reactive<Record<string, Record<string, string>>>(Object.create(null));
const previewDrafts = createChatDraftStore();
const draft = computed(() => reactive((isPreview.value ? previewDrafts : chatDrafts).get(props.session.id)));
const view = computed(() => conversationView(events.value, running.value));
const entry = computed(() => isPreview.value ? { epoch: 0 } : sessionConnections.get(props.session.id));
const matchingProfiles = computed(() => props.profiles.filter(profile => profile.provider === props.session.provider));
const activeProfile = computed(() => props.session.endpoint_snapshot ?? matchingProfiles.value.find(profile => profile.id === props.session.endpoint_profile_id));
const endpointName = computed(() => activeProfile.value?.name ?? t('Native · isolated configuration'));
const turnBusy = computed(() => view.value.turn === 'running' || view.value.awaitingApproval || !!draft.value.pending);
const attachments = ref<Attachment[]>([]), uploading = ref(false);
const caret = ref(0), commandIndex = ref(0), commandsDismissed = ref(false);
/** Suggestions exist only while a command is being typed and the client
 * advertised some. A client with no commands never opens a menu. */
const commandQuery = computed(() => commandsDismissed.value ? undefined : slashQuery(draft.value.text, caret.value));
const commandMatches = computed(() => commandQuery.value ? matchCommands(view.value.commands, commandQuery.value.term) : []);
const commandsOpen = computed(() => commandMatches.value.length > 0);
// A changed filter is a different list, so the highlight starts over instead of
// pointing at a stale row that may no longer exist.
watch(() => commandMatches.value.join('\u0000'), () => { commandIndex.value = 0; });
/**
 * `@path` completion, backed by the workspace index rather than this pane's
 * own knowledge — the composer has never loaded a file tree.
 *
 * Only one suggestion list is ever open: a slash command and a mention cannot
 * both be under the caret, and letting both claim the arrow keys would make
 * neither work.
 */
const mentionHits = ref<SearchHit[]>([]);
const mentionsDismissed = ref(false);
const mentionSearch = createFileSearch();
const mentionTarget = computed(() => (mentionsDismissed.value || commandsOpen.value) ? undefined : mentionQuery(draft.value.text, caret.value));
watch(() => mentionTarget.value?.term, term => {
  if (term === undefined) { mentionSearch.cancel(); mentionHits.value = []; return; }
  mentionSearch.search(props.session.workspace_id, term, results => { mentionHits.value = results?.files ?? []; });
});
const mentionsOpen = computed(() => !!mentionTarget.value && mentionHits.value.length > 0);
const mentionIndex = ref(0);
watch(() => mentionHits.value.map(hit => hit.path).join('\u0000'), () => { mentionIndex.value = 0; });
watch(() => draft.value.text, () => { mentionsDismissed.value = false; });
onBeforeUnmount(() => mentionSearch.cancel());
function chooseMention(hit: SearchHit | undefined) {
  const query = mentionTarget.value; if (!query || !hit) return;
  const next = applyMention(draft.value.text, query, hit.path);
  draft.value.text = next.text; mentionsDismissed.value = false; mentionIndex.value = 0; mentionHits.value = [];
  void nextTick(() => {
    const input = composerInput.value; if (!input) return;
    input.focus(); input.setSelectionRange(next.caret, next.caret); caret.value = next.caret;
  });
}

const fileInput = ref<HTMLInputElement>();
const OTHER_ANSWER = '__agentdock_other__';
const canSend = computed(() => !isPreview.value && supported.value && mode.value === 'structured' && !loading.value && !actionBusy.value && !turnBusy.value && streamState.value === 'connected' && !uploading.value && canSendMessage(draft.value.text, attachments.value));
const selectedEffort = ref('');
/** A launch flag for this session only; it never rewrites a shared host configuration. */
const canTuneModel = computed(() => props.session.provider !== 'terminal');
/** The client's own model list for this session, empty until it announces one. */
/**
 * The model catalog, from the live client when it has spoken and from its
 * endpoint profile before that.
 *
 * A client only publishes its models once it is running, so a session that has
 * not started had nothing to offer. Claude papered over this with a hardcoded
 * list of levels; Codex publishes them per model, so the same guess would have
 * been an invention — and the control simply did not appear, which is the
 * asymmetry this removes. The profile endpoint asks the same client for the
 * same catalog without a session, so the answer is observed either way.
 */
const profileCatalog = ref<NativeModel[]>([]);
let catalogFetched = '';
watch([() => view.value.ready, () => activeProfile.value?.id, () => props.session.endpoint_profile_id], async () => {
  const id = props.session.endpoint_profile_id;
  // The live catalog supersedes this, so there is nothing to ask for once ready.
  if (view.value.ready || !id || catalogFetched === id) return;
  catalogFetched = id;
  try { profileCatalog.value = (await request<{ models?: NativeModel[] }>(`/endpoint-profiles/${id}/models`)).models ?? []; }
  // Staying silent is right: this only ever adds a control, and failing to add
  // one is the behaviour that already existed.
  catch { profileCatalog.value = []; }
}, { immediate: true });

const models = computed(() => view.value.models.length ? view.value.models : profileCatalog.value);
// Before the client speaks, the model in use is the profile's, or the one the
// catalog marks as its default — the same answer the client would give.
const currentModel = computed(() => view.value.model ?? activeProfile.value?.model
  ?? (view.value.ready ? undefined : models.value.find(entry => entry.isDefault)?.id) ?? undefined);
const currentModelEntry = computed(() => models.value.find(entry => entry.id === currentModel.value));
const modelName = computed(() => currentModelEntry.value?.name ?? currentModel.value ?? t('Client default'));
const canPickModel = computed(() => canTuneModel.value && models.value.length > 0 && !isPreview.value && !actionBusy.value && !turnBusy.value && view.value.ready && running.value);
const effortChoices = computed(() => {
  const provider = props.session.provider;
  if (!canTuneModel.value || provider === 'terminal') return [];
  // Once the client has published its models, the levels come from the model
  // actually in use: one that supports none gets no depth control at all.
  const entry = currentModelEntry.value;
  if (entry) return entry.efforts ?? [];
  return modelEfforts(provider, activeProfile.value?.model ?? undefined, undefined, selectedEffort.value || activeProfile.value?.effort || undefined);
});
/** Stop 0 leaves the depth to the endpoint; the rest are the advertised levels,
 * so dragging never invents a level the client did not offer. */
const effortStops = computed(() => ['', ...effortChoices.value]);
const effortIndex = computed(() => Math.max(0, effortStops.value.indexOf(liveEffort.value)));
const liveEffort = computed(() => view.value.effort ?? selectedEffort.value);
const effortName = computed(() => liveEffort.value ? effortLabel(liveEffort.value) : t('Client default'));
/** Track fill, as a fraction, for the coloured part left of the thumb. */
const effortFill = computed(() => effortStops.value.length < 2 ? 0 : effortIndex.value / (effortStops.value.length - 1));
function dragEffort(event: Event) {
  const stop = effortStops.value[Number((event.target as HTMLInputElement).value)];
  if (stop === undefined) return;
  selectedEffort.value = stop;
  // Stop 0 means "leave it to the client"; there is nothing to send for that.
  if (stop) void applyEffort(stop);
}
/** The top third of whatever this client offers, so the flourish marks the
 * genuinely expensive end rather than a level name hardcoded here. */
const effortVivid = computed(() => !!liveEffort.value && effortStops.value.length > 2 && effortIndex.value >= effortStops.value.length - 2);
const endpointMenu = ref<InstanceType<typeof ChipMenu>>();
const modelMenu = ref<InstanceType<typeof ChipMenu>>();
/** Applies immediately: both clients switch in place, so there is no context
 * boundary to confirm and no bridge restart to warn about. */
/**
 * How tools get approved, for the running session.
 *
 * The list comes from the client — Codex has no plan or accept-edits — and the
 * current value is the one the client reports rather than the one last asked
 * for, so this never claims a mode the client is not in.
 */
const PERMISSION_LABELS: Record<string, string> = { ask: 'Ask every time', plan: 'Plan first', accept_edits: 'Accept edits', danger: 'Never ask' };
const permissionChoices = computed(() => view.value.permissionModes ?? []);
const livePermission = computed(() => view.value.permissionMode ?? '');
const permissionName = computed(() => livePermission.value ? t(PERMISSION_LABELS[livePermission.value] ?? livePermission.value) : t('Client default'));
const permissionDanger = computed(() => livePermission.value === 'danger');
const permissionMenu = ref<InstanceType<typeof ChipMenu>>();
async function pickPermission(mode: string) {
  permissionMenu.value?.close(true);
  if (!canConfigure.value || mode === livePermission.value) return;
  const session = props.session.id;
  actionBusy.value = true; error.value = '';
  try { await request('/sessions/' + encodeURIComponent(session) + '/conversation/permission', json('POST', { mode })); }
  catch (cause) { if (props.session.id === session && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === session) actionBusy.value = false; }
}
async function pickModel(id: string) {
  modelMenu.value?.close(true);
  if (!canPickModel.value || id === currentModel.value) return;
  const session = props.session.id;
  actionBusy.value = true; error.value = '';
  try {
    // Only send a depth when one was actually chosen; otherwise the client
    // keeps whatever it is already using.
    await request('/sessions/' + encodeURIComponent(session) + '/conversation/model', json('POST', selectedEffort.value ? { model: id, effort: selectedEffort.value } : { model: id }));
    // The client answers on the event stream, not in this response: a refusal
    // arrives moments later as an error. Some endpoints reject the availability
    // probe a live switch makes, and for those the only way onto that model is
    // to start with it — which is offered rather than done silently, because it
    // begins a new context.
    await new Promise(resolve => setTimeout(resolve, 2500));
    if (props.session.id === session && mounted.value && view.value.model !== id) launchFallback.value = id;
  } catch (cause) { if (props.session.id === session && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === session) actionBusy.value = false; }
}
/**
 * Timeline context menu.
 *
 * Clearing is the client's own /clear, so it only appears when this client
 * advertised that command; AgentDock never invents one.
 */
const timelineMenu = ref<{ x: number; y: number; text: string } | null>(null);
// The collapsed summary shows where a subagent is now, not where it began.
function lastLine(activity: string) {
  const lines = activity.split('\n').filter(line => line.trim() && line !== '…');
  const last = lines[lines.length - 1] ?? '';
  return last.length > 80 ? `${last.slice(0, 79)}…` : last;
}
const clearAvailable = computed(() => canClearContext({ preview: isPreview.value, running: running.value, ready: view.value.ready, busy: turnBusy.value || actionBusy.value, connected: streamState.value === 'connected', commands: view.value.commands }));
function openTimelineMenu(event: MouseEvent) {
  if (isPreview.value) return;
  event.preventDefault();
  timelineMenu.value = { x: event.clientX, y: event.clientY, text: String(window.getSelection() ?? '') };
}
function closeTimelineMenu() { timelineMenu.value = null; }
const timelineMenuStyle = computed(() => timelineMenu.value
  ? { left: Math.min(timelineMenu.value.x, window.innerWidth - 210) + 'px', top: Math.min(timelineMenu.value.y, window.innerHeight - 170) + 'px' }
  : {});
async function copyFromTimeline(value: string) {
  closeTimelineMenu();
  if (!value) return;
  try { await navigator.clipboard.writeText(value); }
  catch { error.value = t('Could not copy to the clipboard in this browser.'); }
}
/** Sends the client's own /clear rather than wiping the view locally: the
 * transcript and the client's context have to go together, and only the client
 * can drop its own. */
async function clearContext() {
  closeTimelineMenu();
  if (!clearAvailable.value) return;
  const id = props.session.id, messageId = randomId();
  actionBusy.value = true; error.value = '';
  try { await request('/sessions/' + encodeURIComponent(id) + '/conversation/message', json('POST', { id: messageId, content: '/clear' })); }
  catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
/** A model the live switch could not reach, offered as a launch choice. */
const launchFallback = ref('');
const launchFallbackName = computed(() => models.value.find(entry => entry.id === launchFallback.value)?.name ?? launchFallback.value);
async function applyModelAtLaunch() {
  const id = props.session.id, model = launchFallback.value;
  if (!model || !canConfigure.value) return;
  actionBusy.value = true; error.value = '';
  try {
    const payload = sessionConfigurationPayload(props.session, props.profiles, selectedProfile.value || null, true, { mode: mode.value, ready: view.value.ready, busy: view.value.turn === 'running', awaitingApproval: view.value.awaitingApproval }, { model, effort: selectedEffort.value });
    await request<Session>('/sessions/' + encodeURIComponent(id) + '/configuration', json('PATCH', payload));
    if (props.session.id === id && mounted.value) { launchFallback.value = ''; notice.value = 'Endpoint changed. The next message starts a fresh native context; earlier messages stay visible here only.'; emit('changed'); await load(); }
  } catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
function pickEndpoint(id: string) {
  if (!canConfigure.value) return;
  selectedProfile.value = id; chooseEndpoint(); endpointMenu.value?.close(true);
}
const canConfigure = computed(() => !isPreview.value && capabilities.sessionConfiguration === true && !loading.value && !actionBusy.value && !turnBusy.value && (['stopped', 'failed'].includes(props.session.status) || mode.value === 'structured' && running.value && view.value.ready));
const statusLabel = computed(() => isPreview.value ? 'Read-only UI preview' : loading.value ? 'Loading conversation…' : view.value.awaitingApproval ? 'Waiting for your approval' : view.value.turn === 'running' ? 'Agent is working' : streamState.value === 'error' || streamState.value === 'disconnected' ? 'View disconnected' : mode.value === 'structured' ? 'Ready for your next idea' : 'Native client session');
let generation = 0, readController: AbortController | undefined, stream: ReturnType<typeof createSessionStream> | undefined;

function atBottom() { const el = viewport.value; followBottom.value = !el || el.scrollHeight - el.scrollTop - el.clientHeight < 90; }
async function scrollToLatest(force = false) { await nextTick(); const el = viewport.value; if (el && (force || followBottom.value)) { el.scrollTop = el.scrollHeight; followBottom.value = true; } }
function updateSnapshot(raw: unknown) {
  const snapshot = parseConversationSnapshot(raw);
  const bounded = pruneChatEvents(snapshot.events);
  mode.value = snapshot.mode; running.value = snapshot.running; truncated.value = !!snapshot.truncated || bounded.truncated; events.value = bounded.events;
  acknowledgeDraft(draft.value, snapshot.events);
  if (!snapshot.running) clearAnswers();
  for (const event of snapshot.events) if (event.type === 'approval_resolved') clearAnswers(event.id);
  void scrollToLatest();
}
function attach(id: string, own: number) {
  stream?.dispose();
  const current = () => mounted.value && own === generation && props.session.id === id;
  stream = createSessionStream({
    createSocket: url => new WebSocket(url),
    onState: (state, detail) => { if (current()) { streamState.value = state; streamNotice.value = detail; } },
    onMessage: raw => {
      if (!current() || typeof raw !== 'string') return;
      if (new TextEncoder().encode(raw).byteLength > 16 * 1024 * 1024) throw Error('Invalid conversation response.');
      const message: unknown = JSON.parse(raw);
      if (message && typeof message === 'object' && (message as { type?: string }).type === 'snapshot') { updateSnapshot(message); return; }
      if (!isChatEvent(message)) throw Error('Invalid conversation response.');
      const bounded = pruneChatEvents(appendChatEvent(events.value, message));
      events.value = bounded.events; truncated.value ||= bounded.truncated;
      acknowledgeDraft(draft.value, events.value);
      if (message.type === 'ready') { running.value = true; emit('changed'); }
      if (message.type === 'exit') { running.value = false; emit('changed'); }
      if (message.type === 'approval_resolved') clearAnswers(message.id);
      if (message.type === 'configuration' || message.type === 'exit') clearAnswers();
      // A turn starting is also worth reporting: the session takes its name from
      // the message that opened it, and the sidebar is where that name is read.
      if (message.type === 'turn') emit('changed');
      void scrollToLatest();
    },
  });
  const url = new URL('/api/sessions/' + encodeURIComponent(id) + '/chat/ws', window.location.href); url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  stream.connect(url.href);
}
async function load(consumeOpenIntent = false) {
  const id = props.session.id, own = ++generation;
  readController?.abort(); stream?.dispose(); readController = new AbortController();
  if (!mounted.value || !supported.value) { loading.value = false; return; }
  if (props.previewSnapshot) { updateSnapshot(props.previewSnapshot); loading.value = false; streamState.value = 'connected'; streamNotice.value = ''; error.value = ''; return; }
  loading.value = true; error.value = ''; streamNotice.value = ''; streamState.value = 'connecting';
  try {
    // ensure only consumes the parent shell's explicit open intent. A normal
    // stopped-session remount has no permission to start a process.
    if (consumeOpenIntent) { await sessionConnections.ensure(props.session); if (own !== generation) return; emit('changed'); }
    const snapshot = await request<unknown>('/sessions/' + encodeURIComponent(id) + '/conversation', { signal: readController.signal });
    if (!mounted.value || own !== generation || props.session.id !== id) return;
    updateSnapshot(snapshot);
    // A legacy PTY session is rendered in this shell but its terminal stream
    // belongs to NativeSessionPane. Do not attach the structured socket or
    // turn a view mount into a start request; the user must explicitly enable
    // conversation mode first.
    if (mode.value === 'structured') attach(id, own); else streamState.value = 'disconnected';
  } catch (cause) { if (own === generation && mounted.value && !(cause instanceof Error && cause.name === 'AbortError')) { error.value = t(errorMessage(cause)); streamState.value = 'error'; } }
  finally { if (own === generation) loading.value = false; }
}
watch([() => props.session.id, () => props.session.interaction_mode, () => props.session.configuration_revision, () => entry.value.epoch, () => props.previewSnapshot, mounted, supported], () => {
  clearAnswers();
  wakeAttempted = ''; launchFallback.value = '';
  mode.value = undefined; events.value = []; endpointConfirm.value = false; confirmEnd.value = false; approvalBusy.value = ''; actionBusy.value = false; notice.value = ''; selectedProfile.value = props.session.endpoint_profile_id ?? ''; followBottom.value = true;
  void load(props.consumeOpenIntent !== false && props.session.interaction_mode === 'structured');
});
watch(() => props.session.endpoint_profile_id, id => { if (!endpointConfirm.value) selectedProfile.value = id ?? ''; });
onMounted(() => { mounted.value = true; });
onBeforeUnmount(() => { mounted.value = false; generation++; readController?.abort(); stream?.dispose(); });

/** Focusing the composer is an intent to use this session, unlike restoring a
 * layout. Waking here is what makes the model list and the client's own
 * commands available before the first message rather than after it: both are
 * announced at startup, and starting sends no prompt, so nothing is spent. */
let wakeAttempted = '';
async function wake() {
  if (isPreview.value || !supported.value || running.value || loading.value) return;
  if (props.session.interaction_mode !== 'structured' && mode.value !== 'structured') return;
  const id = props.session.id;
  if (wakeAttempted === id || sessionConnections.get(id).pending) return;
  wakeAttempted = id;
  sessionConnections.requestOpen(id);
  await load(true);
}
async function send() {
  if (!canSend.value) return;
  // This client exposes no commands, so a bare slash command would reach the
  // model as prose. Say so instead of sending something that cannot work.
  const unsupported = unsupportedCommand(draft.value.text, view.value.commands, view.value.commandsAnnounced);
  // Codex has no slash commands in its protocol at all; what its terminal
  // offers as /model and /approvals are the controls beside this box here.
  if (unsupported) { draft.value.notice = undefined; error.value = t('{command} is not a command this client accepts. Model and thinking depth are the controls below; anything else belongs in the native terminal view.', { command: '/' + unsupported }); return; }
  const id = props.session.id, target = draft.value, messageId = randomId();
  // Attachment paths travel with the text, so an accepted receipt covers both.
  const typed = target.text;
  const content = composeMessage(typed, attachments.value);
  attachments.value = [];
  target.pending = { id: messageId, content, text: typed, state: 'sending', profileId: props.session.endpoint_profile_id, configurationRevision: props.session.configuration_revision??0 }; target.notice = undefined; error.value = '';
  await deliver(id, target);
}
async function pickAttachments(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = [...(input.files ?? [])];
  input.value = '';
  await uploadFiles(files);
}
/**
 * Files dropped on, or pasted into, the composer.
 *
 * A pasted screenshot arrives as a file with no name of its own, so one is
 * built from its type rather than sending the browser's "image.png" for every
 * paste and overwriting the last one.
 */
async function pasteFiles(event: ClipboardEvent) {
  const files = [...(event.clipboardData?.files ?? [])];
  if (isPreview.value || !files.length) return;
  // Text on the clipboard still pastes normally; only files are intercepted.
  event.preventDefault();
  await uploadFiles(files.map(file => {
    if (file.name && file.name !== 'image.png') return file;
    const extension = file.type.split('/')[1]?.replace(/[^a-z0-9]/gi, '') || 'bin';
    const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    return new File([file], `pasted-${stamp}.${extension}`, { type: file.type });
  }));
}
async function dropFiles(event: DragEvent) {
  const files = [...(event.dataTransfer?.files ?? [])];
  if (isPreview.value || !files.length) return;
  event.preventDefault();
  await uploadFiles(files);
}
async function uploadFiles(files: File[]) {
  if (isPreview.value || !files.length) return;
  uploading.value = true; error.value = '';
  try {
    for (const file of files) {
      const reason = attachmentError(file, attachments.value.length);
      if (reason) { error.value = t(reason, { count: MAX_ATTACHMENTS_PER_MESSAGE }); break; }
      const saved = await request<Attachment>(
        '/workspaces/' + encodeURIComponent(props.session.workspace_id) + '/attachments?name=' + encodeURIComponent(file.name),
        { method: 'POST', headers: { 'Content-Type': 'application/octet-stream', 'X-AgentDock-Client': 'web' }, body: file },
      );
      attachments.value = [...attachments.value, saved];
    }
  } catch (cause) { error.value = t(errorMessage(cause)); }
  finally { uploading.value = false; }
}
/** Removing a chip only drops the reference; the uploaded file stays in the
 * workspace where the user can see and delete it. */
function removeAttachment(path: string) { attachments.value = attachments.value.filter(item => item.path !== path); }

async function retryDelivery() {
  if (isPreview.value || loading.value || actionBusy.value || streamState.value !== 'connected') return;
  const target = draft.value;
  try { pendingMessageRetry(target, props.session); target.pending!.state = 'sending'; target.notice = undefined; await deliver(props.session.id, target); }
  catch (cause) { error.value = t(errorMessage(cause)); }
}
async function deliver(id: string, target: ChatDraft) {
  const pending = target.pending; if (!pending || isPreview.value) return;
  const { id: messageId, content } = pending;
  error.value = '';
  try {
    const receipt=await request('/sessions/' + encodeURIComponent(id) + '/conversation/message', json('POST', { id: messageId, content, configuration_revision: pending.configurationRevision??0 }));
    acknowledgeReceipt(target,receipt);
    if (target.text === content) target.text = '';
    // A durable receipt can acknowledge an older, already-trimmed message.
    // Missing receipts still need the canonical event; never replay automatically.
    if (target.pending?.id === messageId) { target.pending.state = 'unknown'; target.notice = 'Message accepted. Refresh the conversation if its event has not appeared.'; }
    if (props.session.id === id && mounted.value) { emit('changed'); void load(); }
  } catch (cause) {
    if (target.pending?.id === messageId) {
      if (cause instanceof ApiConnectionError || !(cause instanceof Error) || !('status' in cause) || Number(cause.status) >= 500 || Number(cause.status) < 400) { target.pending.state = 'unknown'; target.notice = 'Delivery is uncertain. Your draft is kept; refresh the conversation before deciding what to do. It will not be sent again automatically.'; }
      else {target.pending = undefined;if(!target.text)target.text=content;}
    }
    if (props.session.id === id && mounted.value) error.value = errorMessage(cause);
  }
}
async function explicitOpen() {
  if (isPreview.value || actionBusy.value || loading.value || !supported.value || mode.value === 'pty' && running.value) return;
  const id = props.session.id; actionBusy.value = true; error.value = '';
  try { await request('/sessions/' + encodeURIComponent(id) + '/conversation/open', json('POST', {})); if (props.session.id === id && mounted.value) { emit('changed'); await load(); } }
  catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
/**
 * Open this conversation in a real terminal, for the interactive commands the
 * structured pipe cannot carry. The new session is a sibling, not a
 * replacement: this pane keeps its history and stays exactly where it is.
 */
async function openInTerminal() {
  if (isPreview.value || actionBusy.value || !terminalReopenAvailable.value) return;
  const id = props.session.id; actionBusy.value = true; error.value = '';
  try {
    const opened = await terminalReopen(props.session);
    if (props.session.id === id && mounted.value) { emit('changed'); emit('openSession', opened); }
  }
  catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
async function interrupt() {
  if (isPreview.value || actionBusy.value || view.value.turn !== 'running') return;
  const id = props.session.id; actionBusy.value = true; error.value = '';
  try { await request('/sessions/' + encodeURIComponent(id) + '/conversation/interrupt', json('POST', {})); if (props.session.id === id && mounted.value) { emit('changed'); await load(); } }
  catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
async function endSession() {
  if (isPreview.value || actionBusy.value || !confirmEnd.value) return;
  const id = props.session.id; actionBusy.value = true; error.value = '';
  try {
    await request('/sessions/' + encodeURIComponent(id) + '/stop', json('POST'));
    sessionConnections.ended(id);
    if (props.session.id === id && mounted.value) { confirmEnd.value = false; emit('changed'); await load(); }
  } catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
function closeSessionMenu(restoreFocus = false) {
  const menu = sessionMenu.value;
  if (!menu) return;
  menu.open = false;
  if (restoreFocus) menu.querySelector<HTMLElement>('summary')?.focus({ preventScroll: true });
}
function requestEnd() { closeSessionMenu(); confirmEnd.value = true; }
function cancelEndpointChange() { endpointConfirm.value = false; selectedProfile.value = props.session.endpoint_profile_id ?? ''; }
function paneKeydown(event: KeyboardEvent) {
  if (event.key !== 'Escape' || event.defaultPrevented || composing.value) return;
  if (sessionMenu.value?.open) { event.preventDefault(); event.stopPropagation(); closeSessionMenu(true); return; }
  if (confirmEnd.value) { event.preventDefault(); event.stopPropagation(); confirmEnd.value = false; composerInput.value?.focus({ preventScroll: true }); return; }
  if (endpointConfirm.value) { event.preventDefault(); event.stopPropagation(); cancelEndpointChange(); composerInput.value?.focus({ preventScroll: true }); return; }
  if (!isPreview.value && view.value.turn === 'running' && !view.value.awaitingApproval) { event.preventDefault(); event.stopPropagation(); void interrupt(); return; }
  if (event.target === composerInput.value) { event.preventDefault(); event.stopPropagation(); composerInput.value?.blur(); }
}
const approvalChoices = (item: Extract<ChatItem, { type: 'approval' }>) => item.choices.filter(choice => ['accept', 'decline', 'cancel'].includes(choice)) as Array<'accept' | 'decline' | 'cancel'>;
function clearAnswers(id?: string) { for (const key of id ? [id] : new Set([...Object.keys(approvalAnswers), ...Object.keys(approvalOther)])) { delete approvalAnswers[key]; delete approvalOther[key]; } }
function answer(id: string, question: string, event: Event) { const value = (event.target as HTMLInputElement).value; (approvalAnswers[id] ??= Object.create(null))[question] = value && value !== OTHER_ANSWER ? [value] : []; if (value !== OTHER_ANSWER && approvalOther[id]) delete approvalOther[id][question]; }
function toggleAnswer(id: string, question: string, value: string, event: Event) { const current = (approvalAnswers[id] ??= Object.create(null))[question] ?? []; approvalAnswers[id][question] = (event.target as HTMLInputElement).checked ? [...new Set([...current, value])] : current.filter(item => item !== value); }
function otherAnswer(id: string, question: string, event: Event) { const value = (event.target as HTMLInputElement).value; (approvalOther[id] ??= Object.create(null))[question] = value; if (value.trim()) (approvalAnswers[id] ??= Object.create(null))[question] = []; }
function incompleteAnswers(item: Extract<ChatItem, { type: 'approval' }>) { const payload = approvalPayloadAnswers(item.questions, approvalAnswers[item.id], approvalOther[item.id]); return item.questions.some(question => !Object.hasOwn(payload, question.id)); }
async function approve(item: Extract<ChatItem, { type: 'approval' }>, decision: 'accept' | 'decline' | 'cancel') {
  if (isPreview.value || approvalBusy.value || item.resolved || !approvalChoices(item).includes(decision) || decision === 'accept' && incompleteAnswers(item)) return;
  const id = props.session.id; approvalBusy.value = item.id; error.value = '';
  const answers = decision === 'accept' ? approvalPayloadAnswers(item.questions, approvalAnswers[item.id], approvalOther[item.id]) : {};
  try { await request('/sessions/' + encodeURIComponent(id) + '/conversation/approval', json('POST', { request_id: item.id, decision, ...(Object.keys(answers).length ? { answers } : {}) })); if (props.session.id === id && mounted.value) { clearAnswers(item.id); await load(); } }
  catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) approvalBusy.value = ''; }
}
async function configure() {
  if (!canConfigure.value || !endpointConfirm.value) return;
  const id = props.session.id; actionBusy.value = true; error.value = '';
  try {
    const payload = sessionConfigurationPayload(props.session, props.profiles, selectedProfile.value || null, true, { mode: mode.value, ready: view.value.ready, busy: view.value.turn === 'running', awaitingApproval: view.value.awaitingApproval }, { effort: selectedEffort.value });
    await request<Session>('/sessions/' + encodeURIComponent(id) + '/configuration', json('PATCH', payload));
    if (props.session.id === id && mounted.value) { endpointConfirm.value = false; selectedEffort.value = ''; notice.value = 'Endpoint changed. The next message starts a fresh native context; earlier messages stay visible here only.'; emit('changed'); await load(); }
  } catch (cause) { if (props.session.id === id && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === id) actionBusy.value = false; }
}
function chooseEndpoint() { endpointConfirm.value = selectedProfile.value !== (props.session.endpoint_profile_id ?? '') || !!selectedEffort.value; }
/** Thinking depth applies in place, exactly like the model. Routing it through
 * the endpoint change would restart the bridge — and an official account refuses
 * a model or depth on that path at all, because those belong to its own client. */
async function applyEffort(level: string) {
  if (!canPickModel.value) return;
  const session = props.session.id;
  actionBusy.value = true; error.value = '';
  try {
    await request('/sessions/' + encodeURIComponent(session) + '/conversation/model', json('POST', { effort: level }));
  } catch (cause) { if (props.session.id === session && mounted.value) error.value = errorMessage(cause); }
  finally { if (props.session.id === session) actionBusy.value = false; }
}
function syncCaret(event: Event) {
  const input = event.target as HTMLTextAreaElement;
  caret.value = input.selectionStart ?? input.value.length;
  if (!input.value.startsWith('/')) commandsDismissed.value = false;
}
function chooseCommand(command: string) {
  const query = commandQuery.value; if (!query) return;
  const next = applyCommand(draft.value.text, query, command);
  draft.value.text = next.text; commandsDismissed.value = false; commandIndex.value = 0;
  void nextTick(() => {
    const input = composerInput.value; if (!input) return;
    input.focus(); input.setSelectionRange(next.caret, next.caret); caret.value = next.caret;
  });
}
function keydown(event: KeyboardEvent) {
  if (composing.value || event.isComposing || event.keyCode === 229) return;
  // While the suggestion list is open it owns the arrows, Tab and Enter, so a
  // highlighted command is completed instead of being sent as literal text.
  if (commandsOpen.value) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      commandIndex.value = moveHighlight(commandIndex.value, commandMatches.value.length, event.key === 'ArrowDown' ? 1 : -1);
      return;
    }
    if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey && !event.repeat)) {
      event.preventDefault(); event.stopPropagation();
      chooseCommand(commandMatches.value[commandIndex.value] ?? commandMatches.value[0]);
      return;
    }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); commandsDismissed.value = true; return; }
  }
  // Mentions take the same keys, and only when no command list is open — the
  // two cannot be under one caret, so they never contend for the arrows.
  if (mentionsOpen.value) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      mentionIndex.value = moveHighlight(mentionIndex.value, mentionHits.value.length, event.key === 'ArrowDown' ? 1 : -1);
      return;
    }
    if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey && !event.repeat)) {
      event.preventDefault(); event.stopPropagation();
      chooseMention(mentionHits.value[mentionIndex.value] ?? mentionHits.value[0]);
      return;
    }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); mentionsDismissed.value = true; return; }
  }
  if (event.key !== 'Enter') return;
  // Shift+Enter keeps the native newline behavior; plain Enter and Ctrl/⌘
  // Enter submit. Repeated keydown is suppressed to avoid duplicate turns.
  if (event.shiftKey || event.repeat) return;
  event.preventDefault(); event.stopPropagation();
  if (!sessionMenu.value?.open && !confirmEnd.value && !endpointConfirm.value) void send();
}
</script>

<template>
  <section :class="['chat-pane', { 'has-tab-menu': !!paneId }]" :aria-label="t('Conversation with {provider}', {provider:providerLabel(session.provider)})" @keydown="paneKeydown">
    <Teleport to="body"><div v-if="timelineMenu" class="chat-timeline-backdrop" @pointerdown="closeTimelineMenu" @contextmenu.prevent="closeTimelineMenu"><nav class="chat-timeline-menu" :style="timelineMenuStyle" role="menu" :aria-label="t('Conversation actions')" @pointerdown.stop @keydown.esc.stop.prevent="closeTimelineMenu"><button v-if="timelineMenu.text" type="button" role="menuitem" @click="copyFromTimeline(timelineMenu!.text)">{{ t('Copy selection') }}</button><button type="button" role="menuitem" @click="closeTimelineMenu(); scrollToLatest(true)">{{ t('Latest message') }}</button><button type="button" role="menuitem" :disabled="loading" @click="closeTimelineMenu(); load()">{{ t('Refresh conversation') }}</button><template v-if="clearAvailable"><hr/><button type="button" role="menuitem" :disabled="actionBusy" :title="t('Runs this client\'s own /clear: the transcript and its context are dropped together.')" @click="clearContext">{{ t('Clear context') }}</button></template></nav></div></Teleport>
    <Teleport v-if="!isPreview" to="body" :disabled="!paneId"><details ref="sessionMenu" :style="tabMenuStyle" :class="['chat-menu','chat-floating-menu', { 'chat-tab-menu': !!paneId }]" @toggle="positionMenu" @keydown.esc.stop.prevent="closeSessionMenu(true)"><summary :aria-label="t('Session actions')" :title="t('Session actions')"><Icon name="more" :size="17" /></summary><nav :style="tabMenuPanelStyle"><div class="chat-menu-status"><span class="chat-status-dot" :class="{working:turnBusy}" />{{ t(statusLabel) }}</div><button @click="closeSessionMenu(); load()"><Icon name="refresh" :size="14" />{{ t('Refresh conversation') }}</button><button @click="closeSessionMenu(); emit('environment',session.id)"><Icon name="settings" :size="14" />{{ t('Session environment') }}</button><button @click="closeSessionMenu(); emit('profiles')"><Icon name="account" :size="14" />{{ t('Manage endpoint profiles') }}</button><button v-if="legacyAvailable" @click="closeSessionMenu(); emit('legacy')"><Icon name="terminal" :size="14" />{{ t('Open native client view') }}</button><button v-if="terminalReopenAvailable" :disabled="actionBusy" :title="t('For interactive commands such as /config, which the conversation view cannot display')" @click="closeSessionMenu(); openInTerminal()"><Icon name="terminal" :size="14" />{{ t('Open this session in a terminal') }}</button><slot name="session-actions" :close-menu="closeSessionMenu" /><button v-if="running" class="chat-end-button" :disabled="actionBusy" @click="requestEnd"><Icon name="stop" :size="14" />{{ t('End session…') }}</button></nav></details></Teleport>
    <div v-if="!supported" class="chat-notice">{{ t('Structured conversation requires an updated backend. Your native session is unchanged.') }}<button v-if="legacyAvailable" class="chat-link" @click="emit('legacy')">{{ t('Open native client view') }}</button></div>
    <div v-if="error" class="chat-alert" role="alert">{{ error }}<button @click="load()">{{ t('Refresh conversation') }}</button></div>
    <div v-if="streamNotice&&!loading" class="chat-notice" role="status">{{ t(streamNotice) }}<button class="chat-link" @click="load()">{{ t('Reconnect view') }}</button></div>
    <div v-if="notice" class="chat-notice">{{ t(notice) }}</div>
    <div v-if="session.native_source_id" class="chat-notice">{{ t('Native history resumes in the client; earlier transcript is not yet displayed here.') }}</div>
    <div v-if="confirmEnd&&!isPreview" class="chat-end-confirm" role="alertdialog" :aria-label="t('End session')"><p>{{ t('End this session? Its background process will stop; history remains.') }}</p><div><button :disabled="actionBusy" @click="endSession">{{ t('End session') }}</button><button :disabled="actionBusy" @click="confirmEnd=false">{{ t('Keep connected') }}</button></div></div>
    <div ref="viewport" class="chat-timeline" role="log" :aria-label="t('Conversation messages')" aria-live="polite" :aria-busy="view.turn==='running'" @contextmenu="openTimelineMenu" @scroll="atBottom">
      <p v-if="truncated" class="chat-history-notice">{{ t('Earlier display history was trimmed. Native history remains managed by the official client.') }}</p>
      <div v-if="!view.items.length" class="chat-welcome"><span class="chat-welcome-mark"><ProviderIcon :provider="session.provider" :size="32" /></span><h3>{{ t('What shall we build?') }}</h3><p>{{ t('A real conversation with your native agent, with room for tools, changes and your next idea.') }}</p><span class="chat-context-chip">{{ endpointName }}</span><p v-if="session.provider==='claude_code'" class="chat-trust-note">{{ t('Claude headless mode skips the interactive workspace-trust prompt. Send messages only for directories you trust; supported tool approvals still come from the native client.') }}</p></div>
      <template v-for="(item,index) in view.items" :key="item.type+':'+index+':'+item.id">
        <article v-if="item.type==='message'" :class="['chat-message',item.role]"><div class="chat-message-label"><ProviderIcon v-if="item.role==='assistant'" :provider="session.provider" :size="15" /><span>{{ item.role==='user'?t('You'):providerLabel(session.provider) }}</span></div><MarkdownContent :text="item.text" /></article>
        <details v-else-if="item.type==='tool'" class="chat-tool"><summary><span :class="['tool-indicator',item.status]">{{ item.status==='completed'?'✓':item.status==='failed'?'!':'↻' }}</span><strong>{{ item.name }}</strong><small>{{ item.activity && item.status==='running' ? lastLine(item.activity) : t(item.status==='running'?(running?'Working…':'Session ended'):item.status==='failed'?'Failed':'Completed') }}</small><Icon class="chat-tool-chevron" name="chevron" :size="14" /></summary><pre v-if="item.activity" class="tool-activity">{{ item.activity }}</pre><pre v-if="item.text">{{ item.text }}</pre><p v-else-if="!item.activity">{{ t('The client did not provide tool output.') }}</p></details>
        <article v-else-if="item.type==='approval'" :class="['chat-approval',{resolved:item.resolved}]"><header><Icon name="info" :size="18" /><strong>{{ item.title }}</strong><span v-if="item.resolved">{{ t('Resolved') }}</span></header><MarkdownContent :text="item.text" /><template v-if="!item.resolved">
          <div v-for="question in item.questions" :key="question.id" class="chat-question"><label :for="'answer-'+session.id+'-'+item.id+'-'+question.id">{{ question.header }} {{ question.question }}</label>
            <div v-if="question.multiSelect&&!question.isSecret&&question.options.length" class="chat-multi-options"><label v-for="option in question.options" :key="option.label"><input type="checkbox" :checked="approvalAnswers[item.id]?.[question.id]?.includes(option.label)??false" :disabled="isPreview||!!approvalBusy" @change="toggleAnswer(item.id,question.id,option.label,$event)" /><span><strong>{{ option.label }}</strong><small v-if="option.description">{{ option.description }}</small></span></label><label v-if="question.isOther" class="chat-other-answer"><span>{{ t('Other answer') }}</span><input type="text" autocomplete="off" :value="approvalOther[item.id]?.[question.id]??''" :disabled="isPreview||!!approvalBusy" @input="otherAnswer(item.id,question.id,$event)" /></label></div>
            <template v-else-if="question.options.length&&!question.isSecret"><select :id="'answer-'+session.id+'-'+item.id+'-'+question.id" :value="approvalAnswers[item.id]?.[question.id]?.[0]??(approvalOther[item.id]?.[question.id] ? OTHER_ANSWER : '')" :disabled="isPreview||!!approvalBusy" @change="answer(item.id,question.id,$event)"><option value="">{{ t('Choose an answer') }}</option><option v-for="option in question.options" :key="option.label" :value="option.label">{{ option.label }}{{ option.description?' — '+option.description:'' }}</option><option v-if="question.isOther" :value="OTHER_ANSWER">{{ t('Other answer') }}</option></select><input v-if="question.isOther" type="text" autocomplete="off" :placeholder="t('Other answer')" :value="approvalOther[item.id]?.[question.id]??''" :disabled="isPreview||!!approvalBusy" @input="otherAnswer(item.id,question.id,$event)" /></template>
            <template v-else><input :id="'answer-'+session.id+'-'+item.id+'-'+question.id" :type="question.isSecret?'password':'text'" :list="!question.isSecret&&question.options.length?'choices-'+session.id+'-'+item.id+'-'+question.id:undefined" autocomplete="off" :value="approvalAnswers[item.id]?.[question.id]?.[0]??''" :disabled="isPreview||!!approvalBusy" @input="answer(item.id,question.id,$event)" /><datalist v-if="!question.isSecret&&question.options.length" :id="'choices-'+session.id+'-'+item.id+'-'+question.id"><option v-for="option in question.options" :key="option.label" :value="option.label">{{ option.description }}</option></datalist></template>
          </div>
          <div class="chat-approval-actions"><button v-for="choice in approvalChoices(item)" :key="choice" :class="{allow:choice==='accept'}" :disabled="isPreview||!!approvalBusy||streamState!=='connected'||(choice==='accept'&&incompleteAnswers(item))" @click="approve(item,choice)">{{ t(choice==='accept'?(item.questions.length?'Submit answers':'Allow once'):choice==='decline'?'Decline':'Cancel turn') }}</button></div><p v-if="!approvalChoices(item).length">{{ t('This native approval needs the original client. No permission was granted automatically.') }}</p>
        </template></article>
        <div v-else-if="item.type==='configuration'" class="chat-boundary"><span>{{ t('New native context') }} · {{ item.profile_name }}</span><p>{{ item.text }}</p><small>{{ t('Earlier display history is not forwarded to the new endpoint.') }}</small></div>
        <p v-else-if="item.type==='error'" class="chat-alert" role="alert">{{ item.text }}</p>
      </template>
      <div v-if="view.turn==='running'" class="chat-working"><ProviderIcon class="chat-working-mark" :provider="session.provider" :size="15"/><span class="chat-working-label">{{ t('Agent is working') }}</span></div>
      <p v-if="draft.pending" class="chat-delivery" role="status">{{ t(draft.notice??(draft.pending.state==='sending'?'Sending your message…':'Waiting for delivery confirmation.')) }}<button v-if="draft.pending.state==='unknown'" class="chat-link" @click="load()">{{ t('Refresh conversation') }}</button><button v-if="draft.pending.state==='unknown'&&!isPreview" class="chat-link" :disabled="loading||actionBusy||streamState!=='connected'" @click="retryDelivery">{{ t('Retry the same message request') }}</button></p>
      <div v-if="supported&&!loading&&mode!=='structured'" class="chat-native-note"><p>{{ t(running?'This session is running in the native client. End it there before enabling chat; it will not be taken over automatically.':'Enable chat explicitly to continue this native session in a conversation view.') }}</p><button class="chat-primary" :disabled="isPreview||actionBusy||running" @click="explicitOpen">{{ t('Enable conversation view') }}</button><button v-if="legacyAvailable" class="chat-link" @click="emit('legacy')">{{ t('Open native client view') }}</button></div>
    </div>
    <button v-if="!followBottom" class="chat-latest" @click="scrollToLatest(true)">↓ {{ t('Latest message') }}</button>
    <form class="chat-composer" @submit.prevent="send">
      <div v-if="launchFallback" class="chat-endpoint-confirm"><strong>{{ t('{model} could not be switched to mid-conversation', { model: launchFallbackName }) }}</strong><p>{{ t('This endpoint refuses the availability check the client makes when switching models live. Starting the session on this model works instead, but begins a new native context: earlier messages stay visible here and are not sent to it.') }}</p><div><button type="button" :disabled="!canConfigure" @click="applyModelAtLaunch">{{ t('Start a new context on this model') }}</button><button type="button" @click="launchFallback=''">{{ t('Cancel') }}</button></div></div><div v-if="endpointConfirm" class="chat-endpoint-confirm"><strong>{{ t('Apply this configuration change?') }}</strong><p>{{ t('Only an idle session can switch. Its current bridge will close; the next message starts a new native context. Old messages stay visible but are never sent to the new endpoint.') }}</p><div><button type="button" :disabled="!canConfigure" @click="configure">{{ t('Confirm endpoint change') }}</button><button type="button" @click="endpointConfirm=false;selectedProfile=session.endpoint_profile_id??'';selectedEffort=''">{{ t('Cancel') }}</button></div></div>
      <div v-if="mentionsOpen" class="chat-commands chat-mentions" role="listbox" :aria-label="t('Workspace files')"><button v-for="(hit,index) in mentionHits" :key="hit.path" type="button" role="option" :aria-selected="index===mentionIndex" :class="{highlighted:index===mentionIndex}" @mousedown.prevent="chooseMention(hit)" @mouseenter="mentionIndex=index"><strong>{{ hit.name }}</strong><small>{{ hit.path }}</small></button><small>{{ t('Files in this workspace · Enter or Tab to insert') }}</small></div>
      <div v-if="commandsOpen" class="chat-commands" role="listbox" :aria-label="t('Native client commands')"><button v-for="(command,index) in commandMatches" :key="command" type="button" role="option" :aria-selected="index===commandIndex" :class="{highlighted:index===commandIndex}" @mousedown.prevent="chooseCommand(command)" @mouseenter="commandIndex=index">/{{ command }}</button><small>{{ t('From this client · Enter or Tab to complete') }}</small></div>
      <textarea ref="composerInput" v-model="draft.text" :aria-label="t('Message your agent')" :placeholder="t('Ask your agent to build, explore, or fix something…')" rows="2" :disabled="!supported||mode!=='structured'||!!draft.pending" @focus="wake" @paste="pasteFiles" @dragover.prevent @drop="dropFiles" @compositionstart="composing=true" @compositionend="composing=false" @keydown="keydown" @keyup="syncCaret" @click="syncCaret" @input="syncCaret" />
      <ul v-if="attachments.length" class="chat-attachments" :aria-label="t('Attached files')"><li v-for="file in attachments" :key="file.path"><Icon name="file" :size="13"/><span class="chat-attachment-name" :title="file.path">{{ file.name }}</span><small>{{ formatBytes(file.bytes) }}</small><button type="button" :aria-label="t('Remove {name}',{name:file.name})" @click="removeAttachment(file.path)"><Icon name="close" :size="12"/></button></li></ul>
      <div class="chat-composer-controls"><input ref="fileInput" class="sr-only" type="file" multiple :disabled="isPreview||uploading" @change="pickAttachments"/><button type="button" class="chat-attach" :aria-label="t(uploading?'Uploading…':'Attach files')" :title="t('Attach files to this workspace')" :disabled="isPreview||uploading||!supported" @click="fileInput?.click()"><Icon name="plus" :size="18"/></button><ChipMenu ref="endpointMenu" class="chat-endpoint" :label="endpointName" :disabled="!canConfigure" :title="canConfigure?t('Endpoint profile'):t('Wait for the current turn and approvals before changing endpoints.')"><template #mark><ProviderIcon :provider="session.provider" :size="14"/></template><nav :aria-label="t('Endpoint profile')"><button type="button" :class="{selected:selectedProfile===''}" @click="pickEndpoint('')">{{ t('Native · isolated configuration') }}</button><button v-for="profile in matchingProfiles" :key="profile.id" type="button" :class="{selected:selectedProfile===profile.id}" @click="pickEndpoint(profile.id)">{{ profile.name }}</button><button v-if="session.endpoint_profile_id&&!matchingProfiles.some(p=>p.id===session.endpoint_profile_id)" type="button" class="selected" disabled>{{ endpointName }}</button></nav></ChipMenu><ChipMenu v-if="canPickModel" ref="modelMenu" class="chat-model" :label="modelName" :title="t('Model')"><nav :aria-label="t('Model')"><button v-for="entry in models" :key="entry.id" type="button" :class="{selected:entry.id===currentModel}" @click="pickModel(entry.id)"><strong>{{ entry.name }}</strong><small v-if="entry.description">{{ entry.description }}</small></button></nav></ChipMenu><ChipMenu v-if="effortChoices.length" class="chat-effort" :class="{vivid:effortVivid}" :label="effortName" :active="!!liveEffort" :disabled="!canConfigure" :title="canConfigure?t('Thinking depth'):t('Wait for the current turn and approvals before changing endpoints.')"><template #mark><Icon v-if="effortVivid" name="spark" :size="12"/></template><div class="chat-effort-body"><header><strong>{{ t('Thinking depth') }}</strong><em>{{ effortName }}</em></header><div class="chat-effort-slider" :style="{'--effort-fill':effortFill}"><span class="chat-effort-track"><i v-for="(stop,index) in effortStops" :key="stop||'default'" :style="{left:effortStops.length<2?'50%':index/(effortStops.length-1)*100+'%'}"/></span><span class="chat-effort-fill"/><span class="chat-effort-thumb"/><input type="range" min="0" :max="effortStops.length-1" step="1" :value="effortIndex" :disabled="!canPickModel" :aria-label="t('Thinking depth')" :aria-valuetext="effortName" @input="dragEffort"/></div><footer><span>{{ t('Faster') }}</span><span>{{ t('Smarter') }}</span></footer></div></ChipMenu><ChipMenu v-if="permissionChoices.length" ref="permissionMenu" class="chat-permission" :class="{danger:permissionDanger}" :label="permissionName" :active="permissionDanger" :disabled="!canConfigure" :title="canConfigure?t('How tools are approved'):t('Wait for the current turn and approvals before changing endpoints.')"><nav :aria-label="t('How tools are approved')"><button v-for="mode in permissionChoices" :key="mode" type="button" :class="{selected:mode===livePermission,danger:mode==='danger'}" @click="pickPermission(mode)"><strong>{{ t(PERMISSION_LABELS[mode] ?? mode) }}</strong><small v-if="mode==='danger'">{{ t('Runs every tool without asking. Anyone who can reach this workspace can too.') }}</small></button></nav></ChipMenu><button v-if="view.turn==='running'" type="button" class="chat-send chat-stop" :aria-label="t('Cancel turn')" :disabled="isPreview||actionBusy" @click="interrupt">■</button><button v-else class="chat-send" type="submit" :aria-label="t('Send message')" :disabled="!canSend">↑</button></div>
      <footer><span :title="t('Ctrl / ⌘ Enter also sends. Esc closes menus first; in the message box it cancels the active reply, not the session.')">{{ t('Enter to send · Shift+Enter for a new line · Esc to dismiss or cancel') }}</span><span class="chat-usage"><ContextRing :usage="view.usage"/><span v-if="view.usage.context_window===undefined&&view.usage.context_tokens!==undefined">{{ t('{count} context tokens',{count:view.usage.context_tokens}) }}</span><span v-else-if="view.usage.context_window===undefined&&view.usage.input_tokens!==undefined">{{ t('{count} input tokens',{count:view.usage.input_tokens}) }}</span></span></footer>
    </form>
  </section>
</template>

<style scoped>
.chat-pane{height:100%;min-height:0;min-width:0;display:flex;flex-direction:column;position:relative;background:var(--surface);color:#273745;container-type:inline-size}.chat-header{display:flex;align-items:center;gap:11px;min-height:64px;flex-shrink:0;padding:10px 18px;border-bottom:1px solid var(--border);background:var(--surface)}.chat-provider{display:grid;place-items:center;width:38px;height:38px;border-radius:12px;background:#F0E9FF;color:#7552B8}.chat-provider.claude_code{background:#FFF0E5;color:#B75B27}.chat-heading{flex:1;min-width:0}.chat-heading strong{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:14px;font-weight:600}.chat-heading small{display:flex;align-items:center;gap:6px;margin-top:5px;font-size:11px;color:var(--muted)}.chat-status-dot{width:6px;height:6px;background:#3ba791;border-radius:50%}.chat-status-dot.working{background:#c9a351}.chat-menu{position:relative}.chat-menu summary{list-style:none;display:grid;place-items:center;width:44px;height:44px;font-size:24px;cursor:pointer;border-radius:10px}.chat-menu summary::-webkit-details-marker{display:none}.chat-menu nav{position:absolute;right:0;top:44px;z-index:20;min-width:205px;background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:6px;box-shadow:0 12px 35px #243b4c20}.chat-menu button{display:block;width:100%;min-height:44px;border:0;background:none;text-align:left;padding:9px;color:var(--ink-soft);border-radius:7px}.chat-menu button:hover,.chat-menu summary:hover{background:var(--teal-soft)}.chat-timeline{min-height:0;flex:1;overflow-y:auto;overscroll-behavior:contain;padding:24px clamp(14px,5cqw,44px)}.chat-welcome{max-width:470px;text-align:center;margin:clamp(20px,6cqh,70px) auto 32px}.chat-welcome-mark{display:grid;place-items:center;width:64px;height:64px;border-radius:22px;background:var(--teal-soft);color:var(--teal);margin:0 auto 18px}.chat-welcome h3{font-size:23px;font-weight:550;letter-spacing:-.6px;margin:0 0 10px}.chat-welcome p{font-size:13px;line-height:1.8;color:var(--ink-soft);max-width:330px;margin:0 auto}.chat-context-chip{display:inline-block;max-width:100%;overflow-wrap:anywhere;margin-top:17px;border:1px solid var(--border);background:#f5f7f9;border-radius:20px;padding:7px 12px;font-size:11px;color:var(--ink-soft)}.chat-message{max-width:820px;margin:0 auto 24px;min-width:0}.chat-message.user{max-width:82%;margin-left:auto;margin-right:0;background:var(--teal-soft);border:1px solid var(--teal-line);border-radius:18px 18px 5px 18px;padding:13px 17px}.chat-message-label{display:flex;align-items:center;gap:7px;color:var(--ink-soft);font-size:11px;margin-bottom:8px;font-weight:600}.chat-message.user .chat-message-label{color:var(--teal-deep)}.chat-message :deep(.chat-markdown){font-size:14px;line-height:1.85;overflow-wrap:anywhere}.chat-message :deep(p){margin:0 0 12px;white-space:pre-wrap}.chat-message :deep(p:last-child){margin-bottom:0}.chat-tool{max-width:820px;margin:0 auto 13px;border:1px solid var(--border);border-radius:11px;background:var(--sunken);overflow:hidden}.chat-tool summary{display:flex;align-items:center;gap:9px;min-height:46px;padding:8px 13px;cursor:pointer;list-style:none}.chat-tool summary::-webkit-details-marker{display:none}.chat-tool strong{font:12px ui-monospace,monospace;flex:1;min-width:0;overflow-wrap:anywhere}.chat-tool small{font-size:10px;color:var(--muted)}.tool-indicator{color:var(--teal)}.tool-indicator.failed{color:var(--danger)}.tool-indicator.running{color:#9c844c}.chat-tool pre{white-space:pre-wrap;overflow-wrap:anywhere;font:11px/1.8 ui-monospace,monospace;padding:13px;margin:0;max-height:360px;overflow:auto;border-top:1px solid var(--border)}.chat-tool p{padding:12px;font-size:12px;color:var(--ink-soft)}.chat-approval{max-width:820px;margin:17px auto;padding:16px;background:#fffaee;border:1px solid #eee3c6;border-radius:13px;font-size:13px;overflow-wrap:anywhere}.chat-approval.resolved{background:#eef8f3;border-color:var(--teal-line);opacity:.8}.chat-approval header{display:flex;align-items:center;gap:8px;color:#86713e}.chat-approval header strong{flex:1}.chat-approval header span{font-size:10px}.chat-approval :deep(p){line-height:1.7}.chat-question{display:flex;flex-direction:column;gap:7px;margin:13px 0}.chat-question select,.chat-question input{font-size:16px;min-height:44px;width:100%;min-width:0;border:1px solid var(--line);border-radius:8px;padding:9px;background:var(--surface)}.chat-approval-actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:12px}.chat-approval-actions button,.chat-primary,.chat-endpoint-confirm button{min-height:44px;padding:8px 15px;border-radius:9px;border:1px solid var(--line);background:var(--surface);color:var(--ink-soft);font-size:12px;cursor:pointer}.chat-approval-actions button.allow,.chat-primary,.chat-endpoint-confirm button:first-child{background:var(--teal);color:white;border-color:var(--teal)}.chat-boundary{text-align:center;margin:28px 0;border-block:1px solid var(--border);padding:17px;color:var(--ink-soft);font-size:11px}.chat-boundary>span{font-weight:600;color:var(--teal)}.chat-boundary p{margin:7px 0;overflow-wrap:anywhere}.chat-boundary small{font-size:10px}.chat-notice,.chat-alert,.chat-history-notice,.chat-delivery,.chat-native-note{padding:10px 16px;font-size:12px;line-height:1.7;overflow-wrap:anywhere}.chat-notice{flex-shrink:0;background:var(--teal-soft);border-bottom:1px solid var(--teal-line);color:var(--teal-deep)}.chat-alert{background:#fff3f5;color:var(--danger-ink);border-radius:8px;margin:8px 12px;flex-shrink:0}.chat-alert button{background:none;border:0;color:inherit;text-decoration:underline;min-height:44px;padding:8px}.chat-history-notice,.chat-delivery{text-align:center;color:var(--ink-soft);background:#f5f7f9;border-radius:8px}.chat-native-note{border:1px solid var(--border);border-radius:13px;text-align:center;margin:18px 0}.chat-link{min-height:44px;padding:8px;border:0;background:none;color:var(--teal);font-size:12px;text-decoration:underline;cursor:pointer}/* The client's own mark turning is the whole indicator: it says which agent
   is thinking as well as that it is, which three neutral dots never did. */
.chat-working{display:flex;align-items:center;gap:8px;color:var(--ink-soft);font-size:11px;margin:15px auto;max-width:820px}.chat-working-mark{animation:chat-spin 2.6s cubic-bezier(.45,.05,.35,1) infinite;transform-origin:50% 50%;will-change:transform}.chat-working-label{animation:chat-breathe 2.6s ease-in-out infinite}@keyframes chat-spin{0%{transform:rotate(0) scale(1);opacity:.75}24%{transform:rotate(155deg) scale(1.09);opacity:1}46%{transform:rotate(183deg) scale(1);opacity:.8}74%{transform:rotate(348deg) scale(1.06);opacity:1}100%{transform:rotate(360deg) scale(1);opacity:.75}}@keyframes chat-breathe{0%,100%{opacity:.6}50%{opacity:1}}.chat-latest{position:absolute;bottom:180px;align-self:center;border:1px solid var(--line);background:var(--surface);color:var(--teal);box-shadow:0 4px 18px #243b4c12;border-radius:30px;min-height:44px;padding:8px 14px;font-size:12px}.chat-composer{flex-shrink:0;margin:0 16px 12px;padding:12px 13px 7px;background:var(--surface);border:1px solid var(--border);border-radius:17px;box-shadow:0 4px 16px #243b4c08;padding-bottom:max(7px,env(safe-area-inset-bottom))}.chat-composer>textarea{display:block;width:100%;resize:vertical;min-height:66px;max-height:200px;border:0;background:none;box-shadow:none;outline:0;font-family:inherit;font-size:14px;line-height:1.7;color:#273745;padding:3px 3px 10px}.chat-composer>textarea::placeholder{color:var(--muted)}.chat-composer-controls{display:flex;align-items:center;gap:8px;min-width:0}/* The chips themselves are ChipMenu; what stays here is only what goes
   inside their panels. */
.chat-endpoint{max-width:min(46%,216px)}
.chat-model{max-width:min(34%,170px)}
.chat-endpoint :deep(.chip-menu-panel),.chat-model :deep(.chip-menu-panel){min-width:190px;max-width:78cqw;max-height:264px;overflow-y:auto;padding:5px}
.chat-model nav button strong{display:block;font-size:12px;font-weight:600}
.chat-model nav button small{display:block;margin-top:2px;font-size:10px;color:var(--muted);white-space:normal;line-height:1.5}
.chat-model nav button{white-space:normal}
.chat-endpoint nav button,.chat-model nav button{display:block;width:100%;min-height:38px;padding:8px 10px;border:0;border-radius:8px;background:none;text-align:left;font-size:12px;color:var(--ink-soft);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;cursor:pointer}
.chat-endpoint nav button:hover:not(:disabled),.chat-model nav button:hover:not(:disabled){background:var(--teal-soft)}
.chat-endpoint nav button.selected,.chat-model nav button.selected{color:var(--teal);font-weight:600;background:var(--teal-soft)}
.chat-effort{max-width:132px}
/* The unattended mode is the one that stops asking, so it is the one that has
   to look different from every other chip rather than merely selected. */
.chat-permission{max-width:150px}
.chat-permission.danger{border-color:#e0a9a2;background:#fdf3f1;color:#a85c4e}
.chat-permission nav button.danger strong{color:#a85c4e}
.chat-permission nav button.danger small{color:#b07a70;display:block;line-height:1.5;margin-top:2px}
.chat-timeline-backdrop{position:fixed;inset:0;z-index:60}
.chat-timeline-menu{position:fixed;min-width:196px;padding:5px;background:var(--surface);border:1px solid var(--border);border-radius:11px;box-shadow:0 14px 38px #243b4c2b}
.chat-timeline-menu button{display:block;width:100%;min-height:32px;padding:7px 10px;border:0;border-radius:7px;background:none;text-align:left;font-size:12px;color:var(--ink-soft);white-space:nowrap;cursor:pointer}
.chat-timeline-menu button:hover:not(:disabled){background:var(--teal-soft);color:var(--teal)}
.chat-timeline-menu button:disabled{opacity:.5;cursor:not-allowed}
.chat-timeline-menu hr{border:0;border-top:1px solid var(--border);margin:4px 6px}
.chat-effort :deep(.chip-menu-panel){padding:13px 14px 10px}
.chat-effort-body{width:218px;max-width:74cqw}

.chat-effort-body header{display:flex;align-items:baseline;justify-content:space-between;gap:8px;margin-bottom:11px}
.chat-effort-body header strong{font-size:12px;font-weight:600;color:#273745}
.chat-effort-body header em{font-style:normal;font-size:12px;font-weight:600;color:var(--teal)}
.chat-effort.vivid .chat-effort-body header em{background:linear-gradient(92deg,#1c7f70,#4f7fd8);-webkit-background-clip:text;background-clip:text;color:transparent}
.chat-effort-body footer{display:flex;justify-content:space-between;margin-top:7px;font-size:10px;color:var(--muted)}
/* A real range input stays on top, invisible: dragging, clicking the track and
   arrow keys keep working while the pill below is free to be styled. */
.chat-effort-slider{position:relative;height:26px;display:flex;align-items:center}
.chat-effort-track{position:absolute;inset:0;border-radius:13px;background:#eceef0}
.chat-effort-track i{position:absolute;top:50%;width:4px;height:4px;margin:-2px 0 0 -2px;border-radius:50%;background:#c2c9ce}
.chat-effort-track i:first-child{margin-left:5px}
.chat-effort-track i:last-child{margin-left:-9px}
.chat-effort-fill{position:absolute;left:0;top:0;bottom:0;border-radius:13px;width:calc(11px + (100% - 22px)*var(--effort-fill));min-width:26px;background:var(--teal);transition:width .16s ease,background .16s ease}
.chat-effort-thumb{position:absolute;top:50%;left:calc(11px + (100% - 22px)*var(--effort-fill));width:22px;height:22px;margin:-11px 0 0 -11px;border-radius:50%;background:var(--surface);box-shadow:0 1px 5px #243b4c2e;transition:left .16s ease}
.chat-effort-slider input{position:absolute;inset:0;width:100%;height:100%;margin:0;opacity:0;cursor:grab;-webkit-appearance:none;appearance:none}
.chat-effort-slider input:active{cursor:grabbing}
.chat-effort-slider input:disabled{cursor:not-allowed}
.chat-effort-slider input:disabled~.chat-effort-fill,.chat-effort-slider input:disabled~.chat-effort-thumb{opacity:.45}
.chat-effort-slider input:focus-visible~.chat-effort-thumb{outline:2px solid var(--focus);outline-offset:2px}
/* The top levels cost real time and money, so they read as charged rather than
   as one more step: a gradient, a lift, and a slow pulse. */
.chat-effort.vivid .chat-effort-fill{background:linear-gradient(92deg,#1c7f70,#3f9fd0 62%,#6b6ee0);box-shadow:0 0 16px #4f8fd855}
.chat-effort.vivid .chat-effort-thumb{box-shadow:0 1px 6px #243b4c33,0 0 0 3px #6b6ee02e;animation:effort-pulse 2.1s ease-in-out infinite}
@keyframes effort-pulse{0%,100%{box-shadow:0 1px 6px #243b4c33,0 0 0 3px #6b6ee02e}50%{box-shadow:0 1px 6px #243b4c33,0 0 0 7px #6b6ee014}}.chat-send{margin-left:auto;flex-shrink:0;width:44px;height:44px;border:0;border-radius:13px;background:var(--teal);color:white;font-size:25px;cursor:pointer}.chat-send:disabled{opacity:.45;cursor:not-allowed}.chat-stop{background:var(--danger);font-size:17px}.chat-composer footer{display:flex;justify-content:space-between;gap:6px;flex-wrap:wrap;font-size:9px;color:var(--muted);margin:7px 4px 0}.chat-endpoint-confirm{padding:12px;border:1px solid #eee3c6;background:#fffaee;border-radius:10px;margin-bottom:10px;font-size:12px;color:#86713e}.chat-endpoint-confirm p{line-height:1.7;font-size:11px}.chat-endpoint-confirm>div{display:flex;flex-wrap:wrap;gap:8px}.chat-pane button:disabled{cursor:not-allowed;opacity:.5}.chat-pane button:focus-visible,.chat-pane summary:focus-visible,.chat-pane select:focus-visible,.chat-pane input:focus-visible,.chat-pane textarea:focus-visible{outline:2px solid var(--focus);outline-offset:3px}.sr-only{position:absolute;width:1px;height:1px;padding:0;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}@container(max-width:480px){.chat-header{padding:8px 12px}.chat-timeline{padding:17px 13px}.chat-message.user{max-width:92%}.chat-composer{margin:0 8px 8px;padding:10px 10px max(7px,env(safe-area-inset-bottom));border-radius:14px}.chat-endpoint{max-width:52%}.chat-composer footer{font-size:9px}.chat-message :deep(.chat-markdown){font-size:14px}.chat-tool small{display:none}.chat-welcome h3{font-size:21px}.chat-menu nav{min-width:190px}}@media(prefers-reduced-motion:reduce){.chat-pane *{scroll-behavior:auto}.chat-working-mark,.chat-working-label{animation:none}.chat-effort-fill,.chat-effort-thumb{transition:none}.chat-effort.vivid .chat-effort-thumb{animation:none}}
.chat-welcome .chat-trust-note{font-size:11px;color:var(--ink-soft);margin-top:16px;line-height:1.8}.chat-composer footer,.chat-tool small,.chat-history-notice,.chat-heading small{color:var(--ink-soft)}
.chat-usage{display:flex;align-items:center;gap:8px;min-width:0}
.chat-commands{display:flex;flex-direction:column;gap:1px;margin-bottom:8px;padding:5px;border:1px solid var(--border);border-radius:10px;background:var(--surface);box-shadow:0 6px 20px #243b4c12;max-height:216px;overflow-y:auto}
.chat-commands button{display:block;width:100%;text-align:left;border:0;border-radius:6px;background:none;padding:7px 9px;font:12px ui-monospace,monospace;color:var(--ink-soft);cursor:pointer}
.chat-commands button.highlighted{background:var(--teal-soft);color:var(--teal)}
.chat-commands small{padding:6px 9px 2px;font-size:9px;color:var(--muted)}
.chat-attach{flex-shrink:0;width:44px;height:44px;display:grid;place-items:center;border:1px solid var(--border);border-radius:13px;background:var(--surface);color:var(--ink-soft);cursor:pointer}
.chat-attach:hover:not(:disabled){background:var(--teal-soft);color:var(--teal);border-color:var(--teal-line)}
.chat-attachments{list-style:none;display:flex;flex-wrap:wrap;gap:7px;margin:0 0 9px;padding:0}
.chat-attachments li{display:flex;align-items:center;gap:6px;max-width:100%;min-width:0;border:1px solid var(--border);border-radius:9px;background:var(--sunken);padding:5px 5px 5px 9px;font-size:11px;color:var(--ink-soft)}
.chat-attachment-name{min-width:0;max-width:190px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.chat-attachments small{color:var(--muted);font-size:10px;white-space:nowrap}
.chat-attachments button{display:grid;place-items:center;width:30px;height:30px;flex-shrink:0;border:0;border-radius:7px;background:none;color:var(--muted);cursor:pointer}
.chat-attachments button:hover{background:#eef1f3;color:var(--ink-soft)}
/* The visible control stays chip-sized while its touch area reaches 44px, so a
   thumb on a phone does not have to hit a 30px dot. */
.chat-attachments button{position:relative}
.chat-attachments button::after{content:'';position:absolute;inset:-7px}
@container(max-width:480px){.chat-attachment-name{max-width:140px}}
/* Subagent progress is a different kind of content from the tool's own input
   and result, so it reads as a distinct block rather than more of the same. */
.tool-activity{background:#f7f5fc;color:#655a80;border-bottom:1px solid var(--border)}
.chat-tool small{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:45%}
/* A mention row shows the file and where it is, since names repeat across a repo. */
.chat-mentions button{display:flex;align-items:baseline;gap:8px}
.chat-mentions strong{font-weight:550;flex-shrink:0}
.chat-mentions small{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;direction:rtl;text-align:right;color:var(--muted)}
.chat-tool-chevron{transform:rotate(90deg)}.chat-tool[open] .chat-tool-chevron{transform:rotate(270deg)}
.chat-end-confirm{flex-shrink:0;background:#fff3f5;color:var(--danger-ink);border-bottom:1px solid #f1dfe4;padding:11px 16px;font-size:12px;line-height:1.7}.chat-end-confirm p{margin:0 0 8px}.chat-end-confirm>div{display:flex;gap:8px;flex-wrap:wrap}.chat-end-confirm button{min-height:44px;padding:8px 13px;border:1px solid #f1dadd;border-radius:8px;background:var(--surface);color:#ae4055}.chat-end-confirm button:first-child{background:var(--danger);color:white;border-color:var(--danger)}.chat-menu .chat-end-button{color:var(--danger)}
.chat-multi-options{display:flex;flex-direction:column;gap:7px}.chat-multi-options>label{display:flex;align-items:center;gap:10px;min-height:44px;border:1px solid #eee3c6;background:var(--surface);border-radius:9px;padding:9px 11px}.chat-multi-options input[type=checkbox]{min-height:0;width:18px;height:18px;accent-color:var(--teal);flex-shrink:0}.chat-multi-options strong{font-weight:500;font-size:13px}.chat-multi-options small{display:block;font-size:11px;color:var(--ink-soft);margin-top:4px;line-height:1.6}.chat-multi-options>.chat-other-answer{display:flex;align-items:stretch;flex-direction:column;font-size:12px}.chat-other-answer input{font-size:16px}
/* Compact density and a single focus ring for the composer. The outline is
 * intentionally on the whole card, so keyboard focus is visible without a
 * distracting rectangle around the textarea alone. */
.chat-message :deep(.chat-markdown){font-size:13px;line-height:1.7}

.chat-composer:focus-within{border-color:var(--focus);box-shadow:0 0 0 3px var(--focus)26,0 4px 16px #243b4c08}
.chat-composer>textarea{font-size:13px;line-height:1.6}
.chat-composer>textarea:focus-visible{outline:0;box-shadow:none}

.chat-header{display:none}
.chat-floating-menu{position:absolute;top:6px;right:9px;z-index:20}
.chat-floating-menu>summary{list-style:none;display:grid;place-items:center;width:40px;height:40px;border-radius:9px;color:var(--ink-soft);font-size:22px;cursor:pointer}
.chat-floating-menu>summary::-webkit-details-marker{display:none}
.chat-floating-menu>summary:hover,.chat-floating-menu[open]>summary{background:var(--teal-soft);color:var(--teal)}
.chat-floating-menu nav{top:40px}
.chat-menu-status{display:flex;align-items:center;gap:6px;padding:8px 9px;border-bottom:1px solid var(--border);font-size:10px;color:var(--ink-soft)}
.chat-timeline{padding-top:52px}
.chat-pane.has-tab-menu .chat-timeline{padding-top:24px}
@container(max-width:480px){.chat-floating-menu{top:4px;right:5px}.chat-floating-menu>summary{width:44px;height:44px}.chat-timeline{padding-top:49px}}
.chat-pane.has-tab-menu .chat-timeline{padding-top:24px}
.chat-tab-menu{position:fixed;top:auto;right:auto;z-index:60;display:inline-flex}
.chat-tab-menu>summary{width:28px;height:28px;font-size:18px;border-radius:6px}
.chat-tab-menu nav{top:32px;right:0}
.chat-menu button>svg{margin-right:7px}.chat-menu summary:focus-visible,.chat-menu button:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
@media(max-width:520px){.chat-tab-menu>summary{width:32px;height:32px;font-size:19px}.chat-tab-menu nav{top:36px}}

/* The composer is deliberately denser than the message timeline.  Keeping
 * the desktop controls compact makes the input feel like a tool bar instead
 * of a second card, while the mobile rule below restores comfortable touch
 * targets. */
.chat-composer{margin:0 12px 8px;padding:8px 9px max(5px,env(safe-area-inset-bottom));border-radius:10px}
.chat-composer>textarea{min-height:48px;max-height:160px;padding:1px 2px 5px}
.chat-composer-controls{gap:5px}
.chat-composer .chat-attach{width:30px;height:30px;min-width:30px;min-height:0;border-radius:8px}
.chat-send{width:30px;height:30px;min-width:30px;min-height:0;border-radius:8px;font-size:17px}
.chat-stop{font-size:13px}
.chat-composer footer{margin:3px 2px 0;font-size:8px;line-height:1.25}

/* Width controls layout; it does not decide how the controls are pointed at. A
   narrow pane on a desktop is still a mouse, so only spacing changes here. */
@container(max-width:480px){
  .chat-composer{margin:0 8px 8px;padding:10px 10px max(7px,env(safe-area-inset-bottom));border-radius:12px}
  .chat-composer>textarea{min-height:66px;max-height:200px;padding:3px 3px 10px}
  
  .chat-endpoint{max-width:52%}
  .chat-composer footer{margin-top:5px;font-size:9px;line-height:1.4}
}

/* Touch targets follow the pointing device, so a phone or tablet gets 44px at
   any pane width and a mouse keeps the compact tool bar. The 16px input font
   is here too: it stops iOS zooming the page on focus. */
@media(pointer:coarse){
  .chat-composer>textarea{font-size:16px}
  .chat-timeline-menu button{min-height:44px}
  /* A finger needs a taller pill to drag along. */
  .chat-effort-body{width:236px}
  .chat-effort-slider{height:34px}
  .chat-effort-track,.chat-effort-fill{border-radius:17px}
  .chat-effort-fill{width:calc(14px + (100% - 28px)*var(--effort-fill));min-width:34px}
  .chat-effort-thumb{left:calc(14px + (100% - 28px)*var(--effort-fill));width:28px;height:28px;margin:-14px 0 0 -14px}
  .chat-composer .chat-attach{width:44px;height:44px;min-width:44px;min-height:44px;border-radius:12px}
  .chat-composer .chat-send{width:44px;height:44px;min-width:44px;min-height:44px;border-radius:12px;font-size:24px}
  .chat-attachments button{width:44px;height:44px}
}
</style>
