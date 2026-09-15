import { test } from 'node:test';
import { acknowledgeReceipt } from './chat-model.ts';
import assert from 'node:assert/strict';
import { acknowledgeDraft, appendChatEvent, approvalPayloadAnswers, conversationView, createChatDraftStore, isChatEvent, markdownBlocks, markdownInline, parseConversationSnapshot, pendingMessageRetry, pruneChatEvents, safeWebUrl, sessionConfigurationPayload } from './chat-model.ts';

const session = { id: 'session', provider: 'codex', status: 'stopped' };
const profiles = [{ id: 'codex-work', provider: 'codex' }, { id: 'claude-work', provider: 'claude_code' }];

test('a durable message receipt acknowledges a trimmed event without replay or losing a newer draft',()=>{
  const draft={text:'newer edit',pending:{id:'request',content:'old message',text:'old message',state:'unknown'}};
  assert.equal(acknowledgeReceipt(draft,{accepted:true,id:'other'}),false);
  assert.equal(acknowledgeReceipt(draft,{accepted:true,id:'request',duplicate:true}),true);
  assert.equal(draft.text,'newer edit');assert.equal(draft.pending,undefined);
});
test('an uncertain request cannot follow a changed configuration revision even with the same profile ID',()=>{
  const draft={text:'keep',pending:{id:'request',content:'keep',state:'unknown',profileId:'codex-work',configurationRevision:1}};
  assert.throws(()=>pendingMessageRetry(draft,{endpoint_profile_id:'codex-work',configuration_revision:2}),/endpoint changed/);
  assert.deepEqual(pendingMessageRetry(draft,{endpoint_profile_id:'codex-work',configuration_revision:1}),{id:'request',content:'keep'});
});

test('native messages and deltas update a stable message without duplicating replayed sequences', () => {
  const events = [
    { seq: 1, type: 'message', id: 'u', role: 'user', text: 'Build it' },
    { seq: 2, type: 'message', id: 'a', role: 'assistant', text: 'Hello' },
    { seq: 3, type: 'message', id: 'a', role: 'assistant', text: ' world', delta: true },
    { seq: 3, type: 'message', id: 'a', role: 'assistant', text: ' world', delta: true },
  ];
  const view = conversationView(events);
  assert.equal(view.items.length, 2); assert.equal(view.items[1].text, 'Hello world');
  assert.equal(appendChatEvent(events, events[2]).length, 4);
  assert.equal(appendChatEvent(events, { seq: 4, type: 'turn', status: 'completed' }).length, 5);
  assert.equal(events.length, 4);
});

test('bounded history retains old pending approvals and latest control anchors in sequence order', () => {
  const controls = [{ seq: 1, type: 'configuration', id: 'config', profile_name: 'Profile', text: 'Boundary' }, { seq: 2, type: 'ready' }, { seq: 3, type: 'turn', status: 'running' }, { seq: 4, type: 'usage', context_tokens: 123 }, { seq: 5, type: 'approval', id: 'pending', title: 'Keep me', text: 'Native approval', choices: ['accept'] }];
  const history = Array.from({ length: 2005 }, (_, index) => ({ seq: 6 + index, type: 'message', id: String(index), role: 'assistant', text: 'fixture' }));
  const bounded = pruneChatEvents([...controls, ...history]);
  assert.equal(bounded.truncated, true); assert.equal(bounded.events.length, 2005);
  assert.deepEqual(bounded.events.slice(0, 5), controls); assert.equal(conversationView(bounded.events, true).awaitingApproval, true); assert.equal(conversationView(bounded.events, true).turn, 'running');
  assert.equal(conversationView(bounded.events, true).usage.context_tokens, 123);
  const resolved = pruneChatEvents([...bounded.events, { seq: 2011, type: 'approval_resolved', id: 'pending' }]);
  assert.ok(!resolved.events.some(event => event.type === 'approval')); assert.equal(conversationView(resolved.events, true).awaitingApproval, false);
  assert.ok(resolved.events.some(event => event.type === 'turn' && event.status === 'running'));
});

test('event byte budget excludes bounded safety anchors but cannot retain unbounded native approvals', () => {
  const approval = { seq: 1, type: 'approval', id: 'safety', title: 'Keep', text: 'x'.repeat(500), choices: ['decline'] };
  const bounded = pruneChatEvents([approval, { seq: 2, type: 'message', id: 'old', role: 'assistant', text: 'x'.repeat(200) }, { seq: 3, type: 'message', id: 'last', role: 'assistant', text: 'tail' }], 2000, 120);
  assert.equal(bounded.events[0], approval); assert.ok(bounded.historyBytes <= 120); assert.ok(bounded.anchorBytes > 500);
  assert.throws(() => pruneChatEvents(Array.from({ length: 33 }, (_, i) => ({ ...approval, id: String(i), seq: i }))), /Too many/);
  assert.throws(() => pruneChatEvents([{ ...approval, text: 'x'.repeat(193 * 1024) }]), /control event/);
  const ended = pruneChatEvents([approval, { seq: 2, type: 'turn', status: 'running' }, { seq: 3, type: 'exit' }, { seq: 4, type: 'message', id: 'last', role: 'assistant', text: 'tail' }], 1, 120);
  assert.equal(conversationView(ended.events).awaitingApproval, false); assert.equal(conversationView(ended.events).turn, 'interrupted');
});

test('tools, approvals, usage and turn status are projections of official events only', () => {
  const events = [
    { seq: 1, type: 'ready', native_session_id: 'native-thread' },
    { seq: 2, type: 'turn', status: 'running' },
    { seq: 3, type: 'tool', id: '__proto__', name: 'read_file', status: 'running', text: 'fixture output' },
    { seq: 4, type: 'tool', id: '__proto__', name: 'read_file', status: 'completed' },
    { seq: 5, type: 'approval', id: 'permission', title: 'Confirm', text: 'Write file?', choices: ['accept', 'decline'], questions: [] },
  ];
  const before = conversationView(events);
  assert.equal(before.ready, true); assert.equal(before.turn, 'running'); assert.equal(before.awaitingApproval, true);
  assert.deepEqual(before.items[0], { ...events[3], text: 'fixture output' });
  const after = conversationView([...events, { seq: 6, type: 'approval_resolved', id: 'permission' }, { seq: 7, type: 'turn', status: 'completed' }, { seq: 8, type: 'usage', input_tokens: 123, context_tokens: 456 }]);
  assert.equal(after.awaitingApproval, false); assert.equal(after.turn, 'completed'); assert.equal(after.usage.context_tokens, 456);
  assert.equal(before.awaitingApproval, true, 'new snapshots do not mutate earlier rendered projections');
  assert.equal('resolved' in events[4], false, 'provider event is not rewritten');
});

test('endpoint boundary retains old display history without making it a new-context prompt', () => {
  const view = conversationView([{ type: 'ready' }, { type: 'message', id: 'old', role: 'user', text: 'old confidential context' }, { type: 'configuration', id: 'change', profile_name: 'New account', text: 'New context' }]);
  assert.equal(view.items[0].text, 'old confidential context'); assert.equal(view.items[1].type, 'configuration'); assert.equal(view.ready, false);
  assert.deepEqual(sessionConfigurationPayload(session, profiles, 'codex-work', true), { endpoint_profile_id: 'codex-work', confirmed: true });
});

test('the command list is only claimed to be known once a client has announced one', () => {
  // Nothing heard yet: the list is empty because the client has not spoken.
  const fresh = conversationView([]);
  assert.deepEqual(fresh.commands, []); assert.equal(fresh.commandsAnnounced, false);
  // A client that announces none is known to have none.
  assert.equal(conversationView([{ type: 'ready' }]).commandsAnnounced, true);
  const advertised = conversationView([{ type: 'ready', commands: ['model', 'usage'] }]);
  assert.deepEqual(advertised.commands, ['model', 'usage']); assert.equal(advertised.commandsAnnounced, true);
  // A new endpoint is a new client, so the previous list stops being an answer
  // about this one until it announces its own.
  const switched = conversationView([{ type: 'ready', commands: ['model'] }, { type: 'configuration', id: 'change', profile_name: 'Other account', text: 'New context' }]);
  assert.deepEqual(switched.commands, []); assert.equal(switched.commandsAnnounced, false);
  // Exit does not invalidate what the client said: it runs the same commands
  // when it starts again.
  const exited = conversationView([{ type: 'ready', commands: ['model'] }, { type: 'exit' }]);
  assert.deepEqual(exited.commands, ['model']); assert.equal(exited.commandsAnnounced, true);
});

test('a model announcement never disturbs the conversation, and a new endpoint clears it', () => {
  const history = [
    { type: 'ready', commands: ['model'] },
    { type: 'settings', models: [{ id: 'opus', name: 'Opus', efforts: ['low', 'high'] }, { id: 'haiku', name: 'Haiku' }] },
    { type: 'message', id: 'u1', role: 'user', text: 'keep me' },
    { type: 'settings', model: 'haiku' },
  ];
  const view = conversationView(history);
  // Switching model is not a context boundary: the transcript is untouched and
  // the client is still ready, unlike an endpoint change.
  assert.equal(view.items.length, 1);
  assert.equal(view.items[0].text, 'keep me');
  assert.equal(view.ready, true);
  assert.equal(view.model, 'haiku');
  assert.deepEqual(view.models.map(entry => entry.id), ['opus', 'haiku']);
  // A model that advertises no levels genuinely has none.
  assert.equal(view.models[1].efforts, undefined);

  // A new endpoint is a different client, so its models are unknown again.
  const switched = conversationView([...history, { type: 'configuration', id: 'c', profile_name: 'Other', text: 'New context' }]);
  assert.deepEqual(switched.models, []);
  assert.equal(switched.model, undefined);
});

test('pruning keeps the model list even when a later settings event only names a selection', () => {
  const pruned = pruneChatEvents([
    { seq: 1, type: 'settings', models: [{ id: 'opus', name: 'Opus' }] },
    ...Array.from({ length: 30 }, (_, index) => ({ seq: index + 2, type: 'message', id: 'm' + index, role: 'user', text: 'x' })),
    { seq: 40, type: 'settings', model: 'opus' },
  ], 5);
  const view = conversationView(pruned.events);
  assert.deepEqual(view.models.map(entry => entry.id), ['opus'], 'the list must survive a selection-only update');
  assert.equal(view.model, 'opus');
});

test('a client that clears its own context clears the transcript with it', () => {
  const view = conversationView([
    { seq: 1, type: 'ready', native_session_id: 'first' },
    { seq: 2, type: 'message', id: 'u1', role: 'user', text: 'earlier work' },
    { seq: 3, type: 'usage', context_tokens: 148581, context_window: 1000000 },
    { seq: 4, type: 'cleared' },
    { seq: 5, type: 'ready', native_session_id: 'second' },
    { seq: 6, type: 'message', id: 'u2', role: 'user', text: 'after clearing' },
  ]);
  // Keeping the old transcript showed a conversation the client can no longer
  // refer to, which is what made /clear look like it had done nothing.
  assert.deepEqual(view.items.map(item => item.text), ['after clearing']);
  assert.equal(view.usage.context_tokens, undefined);
  assert.equal(view.ready, true);

  // The boundary has to survive trimming, or the cleared messages come back.
  const pruned = pruneChatEvents([
    { seq: 1, type: 'message', id: 'old', role: 'user', text: 'earlier work' },
    { seq: 2, type: 'cleared' },
    ...Array.from({ length: 30 }, (_, index) => ({ seq: index + 3, type: 'message', id: 'm' + index, role: 'user', text: 'x' })),
  ], 5);
  assert.ok(pruned.events.some(event => event.type === 'cleared'), 'the clear boundary is an anchor');
  assert.equal(conversationView(pruned.events).items.some(item => item.text === 'earlier work'), false);
});

test('fresh native contexts may reuse message and tool IDs without replacing earlier display history', () => {
  const view = conversationView([{ type: 'message', id: '1', role: 'assistant', text: 'Old provider text' }, { type: 'tool', id: '1', name: 'old_tool', status: 'completed' }, { type: 'configuration', id: 'boundary', profile_name: 'New endpoint', text: 'New native context' }, { type: 'message', id: '1', role: 'assistant', text: 'New provider text' }, { type: 'tool', id: '1', name: 'new_tool', status: 'running' }]);
  assert.equal(view.items.length, 5); assert.equal(view.items[0].text, 'Old provider text'); assert.equal(view.items[1].name, 'old_tool'); assert.equal(view.items[3].text, 'New provider text'); assert.equal(view.items[4].name, 'new_tool');
});

test('stopped runtime snapshot invalidates stale approvals and running turns without sending any response', () => {
  const events = [{ type: 'ready' }, { type: 'turn', status: 'running' }, { type: 'approval', id: 'lost-rpc', title: 'Old request', text: 'Old', choices: ['accept'] }];
  const view = conversationView(events, false);
  assert.equal(view.ready, false); assert.equal(view.turn, 'interrupted'); assert.equal(view.awaitingApproval, false); assert.equal(view.items[0].resolved, true);
  assert.equal('resolved' in events[2], false, 'display invalidation is not an approval sent to the official client');
});

test('exit interrupts visual running state without claiming that a new agent was started', () => {
  const view = conversationView([{ type: 'ready' }, { type: 'turn', status: 'running' }, { type: 'exit' }]);
  assert.equal(view.ready, false); assert.equal(view.exited, true); assert.equal(view.turn, 'interrupted');
});

test('snapshot parsing preserves legacy missing mode and refuses malformed native events', () => {
  assert.deepEqual(parseConversationSnapshot({ running: false, events: [] }), { mode: undefined, running: false, events: [], truncated: false });
  assert.equal(parseConversationSnapshot({ mode: 'pty', running: true, events: [], truncated: true }).mode, 'pty');
  for (const value of [null, {}, { mode: 'unknown', running: false, events: [] }, { running: 'yes', events: [] }, { running: false, events: [{ type: 'message', id: 'x', role: 'system', text: 'bad' }] }]) assert.throws(() => parseConversationSnapshot(value), /Invalid conversation/);
  assert.equal(isChatEvent({ seq: -1, type: 'exit' }), false);
  assert.equal(isChatEvent({ type: 'usage', input_tokens: NaN }), false);
  assert.equal(isChatEvent({ type: 'approval', id: 'a', title: 'x', text: 'y', choices: ['accept'], questions: [{ id: 'q', question: 'Choose', options: [{ label: 'A' }] }] }), true);
  assert.equal(isChatEvent({ type: 'approval', id: 'a', title: 'x', text: 'y', choices: ['accept'], questions: [{ id: 'q', question: 'Choose', options: [null] }] }), false);
});

test('native question responses retain multiple selections, support other input and keep secret answers single-valued', () => {
  const questions = [{ id: 'multi', question: 'Targets', options: [{ label: 'Web' }, { label: 'API' }], multiSelect: true, isOther: true }, { id: 'single', question: 'One', options: [{ label: 'A' }, { label: 'B' }] }, { id: 'secret', question: 'Value', options: [], isSecret: true, multiSelect: true }];
  assert.deepEqual(approvalPayloadAnswers(questions, { multi: ['Web', 'API'], single: ['B', 'A'], secret: ['synthetic-value', 'discard-extra'] }, { multi: 'Mobile' }), { multi: ['Web', 'API', 'Mobile'], single: ['B'], secret: ['synthetic-value'] });
  assert.deepEqual(approvalPayloadAnswers([{ id: 'fixed', question: 'Only listed', options: [{ label: 'A' }] }], { fixed: ['invented'] }), {});
  assert.deepEqual(approvalPayloadAnswers(questions, {}), {});
});

test('native question IDs are safe data properties and unsupported multiSelect representations fail closed', () => {
  const questions = [{ id: '__proto__', question: 'Fixture', options: [], multiSelect: true }];
  const answers = approvalPayloadAnswers(questions, Object.fromEntries([['__proto__', ['A', 'B']]]));
  assert.equal(Object.hasOwn(answers, '__proto__'), true); assert.deepEqual(answers.__proto__, ['A', 'B']);
  assert.equal(isChatEvent({ type: 'approval', id: 'a', title: 'q', text: '', choices: ['accept'], questions: [{ ...questions[0], multiSelect: 'yes' }] }), false);
});

test('drafts are isolated in memory by session and unknown delivery never clears itself', () => {
  const store = createChatDraftStore(), one = store.get('one'), two = store.get('two');
  one.text = 'private draft'; one.pending = { id: 'request-one', content: one.text, text: one.text, state: 'unknown' };
  assert.equal(two.text, ''); assert.equal(two.pending, undefined); assert.equal(store.get('one'), one);
  assert.equal(acknowledgeDraft(one, []), false); assert.equal(one.pending.state, 'unknown');
  assert.equal(acknowledgeDraft(one, [{ type: 'message', id: 'other', role: 'user', text: one.text }]), false);
  assert.equal(acknowledgeDraft(one, [{ type: 'message', id: 'request-one', role: 'assistant', text: one.text }]), false);
  assert.equal(acknowledgeDraft(one, [{ type: 'message', id: 'request-one', role: 'user', text: one.text }]), true);
  assert.equal(one.text, ''); assert.equal(one.pending, undefined);
  assert.equal(store.get('__proto__').text, '');
});

test('delivery acknowledgement does not discard a subsequently edited draft', () => {
  const draft = { text: 'new text', pending: { id: 'old', content: 'old text', text: 'old text', state: 'unknown' } };
  acknowledgeDraft(draft, [{ type: 'message', id: 'old', role: 'user', text: 'old text' }]);
  assert.equal(draft.text, 'new text'); assert.equal(draft.pending, undefined);
});

test('an explicit unknown-delivery retry keeps both the original UUID and content, and refuses a changed account', () => {
  const draft = { text: 'new edit', pending: { id: 'same-request-uuid', content: 'original request', state: 'unknown', profileId: 'original-profile' } };
  assert.deepEqual(pendingMessageRetry(draft, { endpoint_profile_id: 'original-profile' }), { id: 'same-request-uuid', content: 'original request' });
  assert.throws(() => pendingMessageRetry(draft, { endpoint_profile_id: 'other-profile' }), /another account/);
  assert.equal(draft.pending.state, 'unknown'); assert.equal(draft.text, 'new edit');
  assert.throws(() => pendingMessageRetry({ text: '' }, { endpoint_profile_id: null }), /no uncertain/);
});

test('configuration requires explicit confirmation and same provider, allowing only stopped or ready idle structured sessions', () => {
  assert.throws(() => sessionConfigurationPayload(session, profiles, 'codex-work', false), /Confirm/);
  assert.throws(() => sessionConfigurationPayload(session, profiles, 'claude-work', true), /same client/);
  assert.throws(() => sessionConfigurationPayload(session, profiles, 'missing', true), /same client/);
  assert.deepEqual(sessionConfigurationPayload(session, profiles, null, true), { endpoint_profile_id: null, confirmed: true });
  const live = { ...session, status: 'running' }, idle = { mode: 'structured', ready: true, busy: false, awaitingApproval: false };
  assert.throws(() => sessionConfigurationPayload(live, profiles, null, true), /Wait/);
  assert.throws(() => sessionConfigurationPayload(live, profiles, null, true, { ...idle, mode: 'pty' }), /Wait/);
  assert.throws(() => sessionConfigurationPayload(live, profiles, null, true, { ...idle, busy: true }), /Wait/);
  assert.throws(() => sessionConfigurationPayload(live, profiles, null, true, { ...idle, awaitingApproval: true }), /Wait/);
  assert.deepEqual(sessionConfigurationPayload(live, profiles, null, true, idle), { endpoint_profile_id: null, confirmed: true });
});

test('Markdown is a small text AST: headings, code, lists and emphasis never become raw HTML', () => {
  const blocks = markdownBlocks('# Title\n\nA **bold** thought with `code`.\n\n```ts\n<script>alert(1)</script>\n```\n\n- first\n- second\n\n> quote');
  assert.equal(blocks[0].type, 'heading'); assert.equal(blocks[0].level, 1);
  assert.ok(markdownInline(blocks[1].text).some(part => part.type === 'strong' && part.text === 'bold'));
  assert.deepEqual(blocks[2], { type: 'code', language: 'ts', text: '<script>alert(1)</script>' });
  assert.deepEqual(blocks[3], { type: 'list', ordered: false, items: ['first', 'second'] });
  assert.equal(blocks[4].type, 'quote');
  assert.deepEqual(markdownBlocks('<img src=x onerror=alert(1)>'), [{ type: 'paragraph', text: '<img src=x onerror=alert(1)>' }]);
});

test('Markdown links allow explicit credential-free HTTP(S), not scripts, data, native files or automatic images', () => {
  for (const url of ['javascript:alert(1)', 'data:text/html,test', 'file:///etc/passwd', 'https://user:pass@example.invalid', '/api/action']) assert.equal(safeWebUrl(url), undefined);
  assert.equal(safeWebUrl('https://example.invalid/docs'), 'https://example.invalid/docs');
  assert.ok(markdownInline('[unsafe](javascript:bad)').every(part => part.type !== 'link'));
  assert.deepEqual(markdownInline('[docs](https://example.invalid/docs)'), [{ type: 'link', text: 'docs', href: 'https://example.invalid/docs' }]);
  assert.ok(markdownInline('![image](https://example.invalid/pixel.png)').every(part => part.type !== 'image'));
});

test('model and thinking depth are session-only overrides, and never fight a shared host configuration', () => {
  // Omitted fields keep whatever the profile provides, so an unchanged control
  // does not pin the profile's current value onto the session.
  assert.deepEqual(sessionConfigurationPayload(session, profiles, 'codex-work', true, undefined, {}),
    { endpoint_profile_id: 'codex-work', confirmed: true });
  assert.deepEqual(sessionConfigurationPayload(session, profiles, 'codex-work', true, undefined, { effort: 'high' }),
    { endpoint_profile_id: 'codex-work', effort: 'high', confirmed: true });
  assert.deepEqual(sessionConfigurationPayload(session, profiles, 'codex-work', true, undefined, { model: 'gpt-x', effort: 'max' }),
    { endpoint_profile_id: 'codex-work', model: 'gpt-x', effort: 'max', confirmed: true });
  // Whitespace is not a value.
  assert.deepEqual(sessionConfigurationPayload(session, profiles, 'codex-work', true, undefined, { model: '  ', effort: ' ' }),
    { endpoint_profile_id: 'codex-work', confirmed: true });

  // A shared native configuration may still take a session override: it is a
  // launch flag for this session and never rewrites the client's own settings.
  const shared = [{ id: 'host-codex', provider: 'codex', native_config: { source_id: 'codex-default', config_dir: '/home/u/.codex' } }];
  assert.deepEqual(sessionConfigurationPayload(session, shared, 'host-codex', true), { endpoint_profile_id: 'host-codex', confirmed: true });
  assert.deepEqual(sessionConfigurationPayload(session, shared, 'host-codex', true, undefined, { effort: 'high' }),
    { endpoint_profile_id: 'host-codex', effort: 'high', confirmed: true });

  // The existing guards still apply before any override is considered.
  assert.throws(() => sessionConfigurationPayload(session, profiles, 'codex-work', false, undefined, { effort: 'high' }), /Confirm/);
  assert.throws(() => sessionConfigurationPayload({ ...session, status: 'running' }, profiles, 'codex-work', true, undefined, { effort: 'high' }), /Wait for/);
  assert.throws(() => sessionConfigurationPayload(session, profiles, 'claude-work', true, undefined, { effort: 'high' }), /same client/);
});

test('a sent message leaves the composer, even when the wire content was reshaped', () => {
  // Completing a command from the menu leaves a trailing space, and composing
  // trims it. Comparing the composer against the wire content therefore never
  // matched, so the composer kept holding a command it had already sent.
  const command = { text: '/model ', pending: { id: 'm1', content: '/model', text: '/model ', state: 'sending' } };
  assert.equal(acknowledgeDraft(command, [{ type: 'message', id: 'm1', role: 'user', text: '/model' }]), true);
  assert.equal(command.text, '');

  // An attachment header is added to the wire content and was never in the
  // composer at all.
  const attached = { text: 'look at this', pending: { id: 'm2', content: 'Attached file in this workspace:\n- a.png\n\nlook at this', text: 'look at this', state: 'sending' } };
  assert.equal(acknowledgeReceipt(attached, { accepted: true, id: 'm2' }), true);
  assert.equal(attached.text, '');

  // Still only the sent message is cleared.
  const edited = { text: 'a newer thought', pending: { id: 'm3', content: '/model', text: '/model ', state: 'sending' } };
  acknowledgeReceipt(edited, { accepted: true, id: 'm3' });
  assert.equal(edited.text, 'a newer thought');
});
