import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, copyFile, chmod, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { PassThrough } from 'node:stream';
import { StringDecoder } from 'node:string_decoder';
import { codexLaunch, CodexChat } from './chat-codex.mjs';
import { claudeLaunch, ClaudeChat } from './chat-claude.mjs';
import { validateChatInput } from './chat.mjs';
import { ChatBase, jsonLines, redactEvents } from './chat-common.mjs';

async function fixture(provider,run,args=[]){
  const root=await mkdtemp(join(tmpdir(),'agentdock-chat-test-')),program=join(root,'chat-cli.mjs'),log=join(root,'native.jsonl');
  await copyFile(fileURLToPath(new URL('./fixtures/chat-cli.mjs',import.meta.url)),program);await chmod(program,0o755);
  const child=spawn(process.execPath,[fileURLToPath(new URL('./chat.mjs',import.meta.url))],{env:{PATH:process.env.PATH,AGENTDOCK_CHAT_TEST_LOG:log,AGENTDOCK_CHAT_TEST_INHERITED:'kept',AGENTDOCK_SECRET_TEST_CREDENTIAL:'fixture-secret-value'},stdio:['pipe','pipe','pipe']});
  const events=[],waiters=new Set(),decoder=new StringDecoder('utf8');let buffer='',stderr='',all='';
  const exited=once(child,'exit');child.stderr.on('data',chunk=>{stderr+=chunk;});
  child.stdout.on('data',chunk=>{all+=chunk;buffer+=decoder.write(chunk);let i;while((i=buffer.indexOf('\n'))!==-1){events.push(JSON.parse(buffer.slice(0,i)));buffer=buffer.slice(i+1);for(const notify of [...waiters])notify();}});
  const wait=predicate=>new Promise((resolve,reject)=>{
    const check=()=>{const found=events.find(predicate);if(found){clearTimeout(timer);waiters.delete(check);resolve(found);}};
    const timer=setTimeout(()=>{waiters.delete(check);reject(Error('Timed out waiting for fixture event: '+JSON.stringify(events)));},5000);
    waiters.add(check);check();
  });
  const send=message=>child.stdin.write(JSON.stringify(message)+'\n');
  try{
    send({type:'init',provider,program,args,cwd:root});await wait(event=>event.type==='ready');
    await run({send,wait,events,log:async()=>{try{return(await readFile(log,'utf8')).trim().split('\n').filter(Boolean).map(line=>JSON.parse(line));}catch{return[];}}});
    send({type:'shutdown'});await wait(event=>event.type==='exit');
    const result=await Promise.race([exited,new Promise((_,reject)=>{const timer=setTimeout(()=>reject(Error('Bridge did not exit')),2500);timer.unref();})]);
    assert.equal(result[0],0);assert.equal(stderr,'');assert.equal(all.includes('fixture-secret-value'),false);
  }finally{child.kill('SIGTERM');if(child.exitCode===null&&child.signalCode===null)await new Promise(resolve=>{const timer=setTimeout(()=>{child.kill('SIGKILL');resolve();},2000);child.once('exit',()=>{clearTimeout(timer);resolve();});});await rm(root,{recursive:true,force:true});}
}

test('Codex launch preserves native configuration overrides, translates model and extracts resume without adding permission bypass',()=>{
  const options=codexLaunch({cwd:'/fixture',args:['resume','saved-thread','-c','approval_policy="on-request"','-c','model_reasoning_effort="high"','--model','chosen-model','--no-alt-screen']});
  assert.deepEqual(options.args,['app-server','-c','approval_policy="on-request"','-c','model_reasoning_effort="high"']);assert.equal(options.thread.model,'chosen-model');assert.equal(options.resume,'saved-thread');
  assert.throws(()=>codexLaunch({cwd:'/fixture',args:['--full-auto']}));
});
test('Claude native streaming preserves settings, model and native permission intent without SDK auth or bypass',()=>{
  const options=claudeLaunch({cwd:'/fixture',args:['--model','chosen-model','--permission-mode','plan','--effort','high','--resume=saved-session']});
  assert.ok(options.args.includes('plan'));assert.ok(options.args.includes('--effort'));assert.ok(options.args.includes('high'));assert.ok(options.args.includes('user,project,local'));assert.ok(options.args.includes('--permission-prompt-tool'));assert.ok(options.args.includes('stdio'));
  assert.equal(options.resume,'saved-session');assert.ok(!options.args.includes('--bare'));assert.ok(!options.args.includes('bypassPermissions'));
  assert.throws(()=>claudeLaunch({cwd:'/fixture',args:['--dangerously-skip-permissions']}));
});
test('Claude launch accepts a concrete session name for the native client',()=>{
  const options=claudeLaunch({cwd:'/fixture',args:['--name','Review billing']});
  assert.deepEqual(options.args.slice(0,2),['--name','Review billing']);
});

for(const provider of ['codex','claude_code']){
  test(`${provider}: ready performs no prompt; structured deltas and native session IDs appear only after explicit input`,async()=>fixture(provider,async({send,wait,log})=>{
    assert.ok(!(await log()).some(item=>item.method==='turn/start'||item.method==='thread/start'||item.type==='user'));
    send({type:'message',id:'one',content:'simple'});await wait(event=>event.type==='turn'&&event.id==='one'&&event.status==='completed');
    assert.equal((await wait(event=>event.type==='message'&&event.role==='assistant'&&event.delta===false)).text,'Hello 🦊');
    assert.ok((await wait(event=>event.type==='ready'&&event.native_session_id)).native_session_id);
    assert.equal((await wait(event=>event.type==='usage')).output_tokens,4);
  }));
  test(`${provider}: native approvals wait for an explicit decision, never auto-allow or persist new permission rules`,async()=>fixture(provider,async({send,wait,log})=>{
    send({type:'message',id:'approve',content:'approve'});const approval=await wait(event=>event.type==='approval');
    assert.deepEqual(approval.choices,['accept','decline','cancel']);
    assert.ok(!(await log()).some(item=>item.type==='control_response'||(item.id==='approval-1'&&!item.method)));
    send({type:'approval',request_id:approval.id,decision:'decline'});await wait(event=>event.type==='approval_resolved');await wait(event=>event.type==='turn'&&event.status==='completed');
    assert.equal((await wait(event=>event.type==='tool'&&event.status==='failed')).status,'failed');
  }));
  test(`${provider}: native questions preserve answer mapping`,async()=>fixture(provider,async({send,wait})=>{
    send({type:'message',id:'ask',content:'ask'});const approval=await wait(event=>event.type==='approval');assert.equal(approval.questions.length,1);
    send({type:'approval',request_id:approval.id,decision:'accept',answers:{[approval.questions[0].id]:['Brief']}});await wait(event=>event.type==='turn'&&event.status==='completed');
  }));
  test(`${provider}: accept maps to a one-time native decision and cancel interrupts rather than granting a persistent rule`,async()=>fixture(provider,async({send,wait,log})=>{
    send({type:'message',id:'accept',content:'approve'});const first=await wait(event=>event.type==='approval');
    send({type:'approval',request_id:first.id,decision:'accept'});await wait(event=>event.type==='turn'&&event.id==='accept'&&event.status==='completed');
    const records=await log();const reply=records.find(item=>provider==='codex'?item.id==='approval-1'&&!item.method:item.type==='control_response');
    assert.equal(provider==='codex'?reply.result.decision:reply.response.response.behavior,provider==='codex'?'accept':'allow');
    send({type:'message',id:'cancel',content:'approve'});const second=await wait(event=>event.type==='approval'&&event.id!==first.id);
    send({type:'approval',request_id:second.id,decision:'cancel'});await wait(event=>event.type==='turn'&&event.id==='cancel'&&event.status==='interrupted');
    const delivered=(await log()).find(item=>provider==='codex'?item.id==='approval-2'&&!item.method:item.type==='control_response'&&item.response.request_id==='approval-2');
    assert.equal(provider==='codex'?delivered.result.decision:delivered.response.response.interrupt,provider==='codex'?'cancel':true);
  }));
  test(`${provider}: invalid question answers do not settle or auto-approve the request`,async()=>fixture(provider,async({send,wait})=>{
    send({type:'message',id:'ask-invalid',content:'ask'});const approval=await wait(event=>event.type==='approval');
    send({type:'approval',request_id:approval.id,decision:'accept',answers:{}});await wait(event=>event.type==='error'&&event.message.includes('approval response'));
    send({type:'approval',request_id:approval.id,decision:'accept',answers:{[approval.questions[0].id]:['Brief']}});await wait(event=>event.type==='turn'&&event.id==='ask-invalid'&&event.status==='completed');
  }));
  test(`${provider}: interrupt stops the active turn, not the client, and duplicate message IDs do not trigger another model turn`,async()=>fixture(provider,async({send,wait,log})=>{
    send({type:'message',id:'hold',content:'hold'});await wait(event=>event.type==='tool'&&event.id==='hold-1');send({type:'message',id:'hold',content:'hold'});
    send({type:'interrupt'});await wait(event=>event.type==='turn'&&event.id==='hold'&&event.status==='interrupted');
    send({type:'message',id:'after',content:'simple'});await wait(event=>event.type==='turn'&&event.id==='after'&&event.status==='completed');
    const records=await log();assert.equal(records.filter(item=>provider==='codex'?item.method==='turn/start':item.type==='user').length,2);
  }));
  test(`${provider}: unsupported native requests explicitly fail closed`,async()=>fixture(provider,async({send,wait})=>{
    send({type:'message',id:'unknown',content:'unknown'});await wait(event=>event.type==='error'&&/unsupported|not supported/.test(event.message));await wait(event=>event.type==='turn'&&event.status==='failed');
  }));
}

test('Codex resume happens on the first explicit message and permission grants remain turn-scoped',async()=>fixture('codex',async({send,wait,log})=>{
  assert.ok(!(await log()).some(item=>item.method==='thread/resume'));
  send({type:'message',id:'permissions',content:'permissions'});const request=await wait(event=>event.type==='approval');
  send({type:'approval',request_id:request.id,decision:'decline'});await wait(event=>event.type==='turn'&&event.status==='completed');
  const records=await log();assert.equal(records.find(item=>item.method==='thread/resume').params.threadId,'saved-thread');
  assert.deepEqual(records.find(item=>item.id==='approval-1'&&!item.method).result,{permissions:{},scope:'turn'});
},['resume','saved-thread']));
test('Codex file-change and MCP tool items have complete lifecycle events',async()=>fixture('codex',async({send,wait,events})=>{
  send({type:'message',id:'tools',content:'tools'});await wait(event=>event.type==='turn'&&event.status==='completed');
  assert.deepEqual(events.filter(event=>event.type==='tool'&&event.id==='file-1').map(event=>event.status),['running','completed']);
  assert.ok(events.some(event=>event.type==='tool'&&event.id==='file-1'&&event.text.includes('+fixture')));
  assert.ok(events.some(event=>event.type==='tool'&&event.id==='mcp-1'&&event.name==='fixture / read'&&event.status==='completed'));
}));

for(const provider of ['codex','claude_code']){
  test(`${provider}: the client publishes its own models, and choosing one keeps the native context`,async()=>fixture(provider,async({send,wait,events,log})=>{
    const announced=await wait(event=>event.type==='settings'&&event.models?.length);
    // Every entry comes from the client. A model that states no effort levels is
    // offered without any, rather than with a ladder invented here.
    assert.ok(announced.models.every(entry=>typeof entry.id==='string'&&typeof entry.name==='string'));
    assert.ok(announced.models.some(entry=>entry.efforts?.length));
    assert.ok(announced.models.some(entry=>entry.efforts===undefined));
    // A client marks entries it does not offer; showing them would put models
    // in the picker that the account cannot run.
    assert.equal(announced.models.some(entry=>entry.id==='gpt-hidden'),false);
    // A session that never chose a model still runs one, and the depth control
    // reads from the model in use — so the client's own default is reported.
    if(provider==='codex')assert.equal(announced.model,'gpt-fixture');

    send({type:'message',id:'before',content:'simple'});await wait(event=>event.type==='turn'&&event.id==='before'&&event.status==='completed');
    const startsBefore=(await log()).filter(item=>provider==='codex'?item.method==='thread/start':item.type==='control_request'&&item.request?.subtype==='initialize').length;

    const chosen=announced.models[0].id;
    send({type:'model',model:chosen});
    await wait(event=>event.type==='settings'&&event.model===chosen);

    // The guarantee: no context boundary, no restart, no lost session.
    assert.equal(events.some(event=>event.type==='configuration'),false);
    assert.equal(events.some(event=>event.type==='exit'),false);
    const startsAfter=(await log()).filter(item=>provider==='codex'?item.method==='thread/start':item.type==='control_request'&&item.request?.subtype==='initialize').length;
    assert.equal(startsAfter,startsBefore,'the native session was restarted');

    send({type:'message',id:'after',content:'simple'});await wait(event=>event.type==='turn'&&event.id==='after'&&event.status==='completed');
    const records=await log();
    if(provider==='codex')assert.equal(records.filter(item=>item.method==='turn/start').at(-1).params.model,chosen);
    else assert.ok(records.some(item=>item.type==='control_request'&&item.request?.subtype==='set_model'&&item.request.model===chosen));
  }));
  test(`${provider}: a model change is refused while a turn is running rather than applied mid-answer`,async()=>fixture(provider,async({send,wait})=>{
    send({type:'message',id:'hold',content:'hold'});await wait(event=>event.type==='tool'&&event.id==='hold-1');
    send({type:'model',model:'some-model'});
    await wait(event=>event.type==='error'&&/current turn/.test(event.message));
    send({type:'interrupt'});await wait(event=>event.type==='turn'&&event.id==='hold'&&event.status==='interrupted');
  }));
}

test('a refused model change reports the client\'s own reason, not a generic failure',async()=>fixture('claude_code',async({send,wait})=>{
  await wait(event=>event.type==='settings'&&event.models?.length);
  send({type:'model',model:'refused-model'});
  // "Check client settings and version" left the user with nothing to act on
  // when the refusal actually came from the upstream endpoint.
  const error=await wait(event=>event.type==='error');
  assert.match(error.message,/availability probe refused/);
}));

test('input validation rejects malformed controls and oversized prompts without reflecting their content',()=>{
  assert.throws(()=>validateChatInput({type:'message',id:'one',content:'x'.repeat(256*1024+1)}));
  assert.throws(()=>validateChatInput({type:'approval',request_id:'one',decision:'acceptForSession'}));
  assert.throws(()=>validateChatInput({type:'model',model:''}));
  assert.throws(()=>validateChatInput({type:'model',model:'ok','effort':'VERY-HIGH'}));
  assert.throws(()=>validateChatInput({type:'init',provider:'codex',program:'codex',cwd:'relative',args:[]},true));
});
test('native lines are UTF-8 safe, bounded, and reject incomplete streams',async()=>{
  const input=new PassThrough(),events=[],errors=[];const stop=jsonLines(input,value=>events.push(value),value=>errors.push(value),{maxLine:64,lineTimeout:25});
  const bytes=Buffer.from('{"text":"🦊"}\n');input.write(bytes.subarray(0,10));input.write(bytes.subarray(10));assert.equal(events[0].text,'🦊');
  input.write('x'.repeat(65));assert.equal(errors.length,1);stop();input.destroy();
  const partial=new PassThrough();await new Promise(resolve=>{jsonLines(partial,()=>{},message=>{assert.match(message,/timed out/);resolve();},{lineTimeout:10});partial.write('{');});partial.destroy();
});
test('credential-shaped environment values never appear in structured bridge events',()=>{
  const events=[],emit=redactEvents({AGENTDOCK_SECRET_TEST:'private-fixture',PATH:'/bin'},event=>events.push(event));
  emit({type:'error',message:'Diagnostic private-fixture'});assert.equal(events[0].message,'Diagnostic [redacted]');
});
test('oversized native question option lists fail closed before emitting an incompatible approval event',()=>{
  for(const Type of [CodexChat,ClaudeChat]){
    const events=[],sent=[],chat=new Type({},event=>events.push(event));chat.active={id:'active'};chat.nativeSessionId='fixture-thread';chat.port={send:message=>sent.push(message)};
    const questions=[{id:'choice',question:'Select',options:Array.from({length:33},(_,index)=>({label:String(index)}))}];
    chat.request(Type===CodexChat?{id:'request',method:'item/tool/requestUserInput',params:{threadId:'fixture-thread',questions}}:{type:'control_request',request_id:'request',request:{subtype:'can_use_tool',tool_name:'AskUserQuestion',input:{questions}}});
    assert.equal(events.some(event=>event.type==='approval'),false);assert.equal(events.some(event=>event.type==='error'),true);assert.equal(sent.length,1);
  }
});
test('an idle message limit rejection closes the host pending turn, but busy rejection never completes another turn',()=>{
  const events=[],chat=new ChatBase({},event=>events.push(event));chat.ready=true;chat.seen=new Map(Array.from({length:4096},(_,index)=>[String(index),'hash']));
  assert.equal(chat.begin({id:'over-limit',content:'new'}),undefined);assert.ok(events.some(event=>event.type==='turn'&&event.id==='over-limit'&&event.status==='failed'));
  events.length=0;chat.active={id:'still-running'};chat.begin({id:'another',content:'new'});assert.equal(events.some(event=>event.type==='turn'),false);assert.equal(chat.active.id,'still-running');
});

test('context size comes from the live context, not the turn-cumulative totals', () => {
  const emitted = [];
  const chat = Object.create(ClaudeChat.prototype);
  chat.emit = event => emitted.push(event);
  chat.active = { id: 'turn-1' };
  chat.tools = new Map();
  chat.approvals = new Map();
  chat.closed = false;
  const usageEvents = () => emitted.filter(event => event.type === 'usage');

  // Each assistant message describes one API call: this is the live context.
  chat.notification({ type: 'assistant', message: { id: 'a1', content: [], usage: { input_tokens: 500, cache_read_input_tokens: 40000 } } });
  assert.equal(usageEvents().at(-1).context_tokens, 40500);
  chat.notification({ type: 'assistant', message: { id: 'a2', content: [], usage: { input_tokens: 700, cache_read_input_tokens: 41000 } } });
  assert.equal(usageEvents().at(-1).context_tokens, 41700);

  // The result totals every call in the turn. Deriving context from them would
  // report ~81k here — roughly double the real occupancy — and would climb with
  // each extra tool round.
  chat.notification({
    type: 'result', subtype: 'success',
    usage: { input_tokens: 1200, cache_read_input_tokens: 81000, output_tokens: 900 },
    modelUsage: { 'claude-x': { contextWindow: 200000 } },
  });
  const last = usageEvents().at(-1);
  assert.equal(last.context_tokens, undefined, 'the cumulative total must not overwrite the live context');
  assert.equal(last.context_window, 200000, 'the window is still taken from the result');
});

test('context window is forwarded only when the native client states it', () => {
  const emitted = [];
  const chat = Object.create(ChatBase.prototype);
  chat.emit = event => emitted.push(event);
  // Claude reports its window per model; Codex reports one per thread.
  chat.usage({ input_tokens: 10, output_tokens: 4 }, 50000, 200000);
  assert.deepEqual(emitted.at(-1), { type: 'usage', input_tokens: 10, output_tokens: 4, context_tokens: 50000, context_window: 200000 });
  // A client that never states a window leaves the field out entirely, so no
  // percentage can be derived from an assumed model size.
  chat.usage({ input_tokens: 10 }, 50000, undefined);
  assert.equal('context_window' in emitted.at(-1), false);
  for (const invalid of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, '200000', null]) {
    chat.usage({ input_tokens: 1 }, 5, invalid);
    assert.equal('context_window' in emitted.at(-1), false, String(invalid));
  }
  // No usable numbers at all means no event rather than an empty one.
  const before = emitted.length;
  chat.usage(undefined, undefined, undefined);
  assert.equal(emitted.length, before);
});

test('Claude subagent output reaches its parent tool card instead of the main reply',()=>{
  const events=[];
  const chat=Object.create(ClaudeChat.prototype);
  chat.emit=event=>events.push(event);
  chat.tools=new Map([['task-1','Task']]);
  chat.active={id:'m1',interrupted:false};
  chat.assistantId='assistant-1';

  // Main-agent text: the ordinary path, unchanged.
  chat.notification({type:'assistant',message:{id:'assistant-1',content:[{type:'text',text:'Delegating.'}]}});
  // Subagent text and a nested tool call, both carrying the parent's id.
  chat.notification({type:'assistant',parent_tool_use_id:'task-1',message:{id:'sub-1',content:[
    {type:'text',text:'Scanning the bridge.'},{type:'tool_use',id:'nested-1',name:'Grep',input:{}},
  ]}});

  const messages=events.filter(event=>event.type==='message');
  // The original `return` existed because subagent text overwrote the main
  // reply. Nothing the subagent says may become a message of its own.
  assert.deepEqual(messages.map(event=>event.text),['Delegating.']);

  const progress=events.filter(event=>event.type==='tool'&&event.activity!==undefined);
  assert.equal(progress.length,1);
  assert.equal(progress[0].id,'task-1','progress must attach to the parent card');
  assert.equal(progress[0].name,'Task');
  assert.equal(progress[0].status,'running');
  assert.match(progress[0].activity,/Scanning the bridge\./);
  assert.match(progress[0].activity,/→ Grep/);
  // A nested tool must not open a second top-level card.
  assert.ok(!events.some(event=>event.type==='tool'&&event.id==='nested-1'));
  // And progress never carries `text`, which would clobber the tool's input.
  assert.equal(progress[0].text,undefined);
});

test('subagent output for an unknown parent is dropped rather than inventing a card',()=>{
  const events=[];
  const chat=Object.create(ClaudeChat.prototype);
  chat.emit=event=>events.push(event);chat.tools=new Map();chat.active={id:'m1',interrupted:false};
  chat.notification({type:'assistant',parent_tool_use_id:'never-announced',message:{content:[{type:'text',text:'orphan'}]}});
  assert.deepEqual(events,[]);
});

test('a model announcement keeps the marker naming the configuration default',()=>{
  const events=[];
  const chat=Object.create(ChatBase.prototype);
  chat.emit=event=>events.push(event);
  chat.settings([
    {id:'gpt-5.5',name:'GPT-5.5',efforts:['low','high']},
    {id:'gpt-6-astra',name:'GPT-6-Astra',isDefault:true,efforts:['low','max']},
  ],undefined,undefined);
  const rows=events.at(-1).models;
  // Dropping this here is what left a not-yet-started Codex session unable to
  // offer a thinking depth: with no model named, there was nothing to read
  // levels from, and inventing a list would have been a guess.
  assert.deepEqual(rows.filter(row=>row.isDefault).map(row=>row.id),['gpt-6-astra']);
  assert.deepEqual(rows.find(row=>row.id==='gpt-6-astra').efforts,['low','max']);
  // Only a literal true marks a default; anything else leaves the row unmarked.
  chat.settings([{id:'x',name:'X',isDefault:'yes'}],undefined,undefined);
  assert.equal(events.at(-1).models[0].isDefault,undefined);
});
