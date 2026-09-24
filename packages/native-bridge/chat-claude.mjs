import { randomUUID } from 'node:crypto';
import { ChatBase, NativeProcess, clip, nativeId, MAX_TEXT } from './chat-common.mjs';

/**
 * AgentDock's names for how tools get approved, and Claude Code's.
 *
 * Two vocabularies because two clients: the names sent over this pipe have to
 * mean the same thing for Codex, whose approval model is not Claude's. Mapping
 * here keeps the translation next to the client it translates for.
 */
const CLAUDE_PERMISSION={ask:'default',plan:'plan',accept_edits:'acceptEdits',danger:'bypassPermissions'};
const AGENTDOCK_PERMISSION=Object.fromEntries(Object.entries(CLAUDE_PERMISSION).map(([ours,theirs])=>[theirs,ours]));
// The client calls the ask-every-time mode `default` on current builds and
// `manual` on older ones. Both mean the same thing to a person.
AGENTDOCK_PERMISSION.manual='ask';

export function claudeLaunch(job) {
  const args=[];let resume=job.resume_id,settingSources=false,sessionId,model,effort;
  const values=new Set(['--model','--permission-mode','--permission-prompts','--settings','--setting-sources','--append-system-prompt','--system-prompt','--agent','--agents','--effort','--max-budget-usd','--fallback-model','--allowedTools','--allowed-tools','--disallowedTools','--disallowed-tools','--tools','--add-dir','--plugin-dir','--mcp-config','--name','-n']);
  for(let i=0;i<job.args.length;i++) {
    const arg=job.args[i],equal=arg.indexOf('='),flag=equal>0?arg.slice(0,equal):arg;
    const value=()=>{const result=equal>0?arg.slice(equal+1):job.args[++i];if(typeof result!=='string')throw Error('Missing Claude Code launch option value.');return result;};
    if(flag==='--resume'||flag==='-r'){const id=value();if(!nativeId(id)||(resume&&resume!==id))throw Error('Invalid or conflicting Claude Code resume ID.');resume=id;}
    else if(flag==='--session-id'){sessionId=value();if(!nativeId(sessionId))throw Error('Invalid Claude Code session ID.');args.push(flag,sessionId);}
    else if(flag==='--input-format'||flag==='--output-format')value();
    else if(flag==='--permission-prompt-tool'){if(value()!=='stdio')throw Error('Custom permission prompt tools are unsupported in structured mode.');}
    else if(['--print','-p','--verbose','--include-partial-messages','--replay-user-messages'].includes(flag))continue;
    else if(values.has(flag)) {
      const next=value();if(flag==='--permission-mode'&&next==='bypassPermissions')throw Error('Structured mode does not enable permission bypass.');
      if(flag==='--model')model=next;
      if(flag==='--effort')effort=next;
      if(flag==='--setting-sources')settingSources=true;args.push(flag,next);
    }else if(['--strict-mcp-config','--disable-slash-commands'].includes(flag))args.push(flag);
    else throw Error('Unsupported Claude Code launch option in structured mode.');
  }
  if(resume&&sessionId&&resume!==sessionId)throw Error('Conflicting Claude Code session identifiers.');
  if(resume)args.push('--resume',resume);
  // Permit switching into the unattended mode later without permitting it now:
  // the client refuses `set_permission_mode` to bypassPermissions unless it was
  // launched this way, and this flag is the one that allows rather than the one
  // that enables. The mode in force is stated explicitly once the session is
  // running, so it is never inherited from whatever this flag implies.
  args.push('--allow-dangerously-skip-permissions');
  if(!settingSources)args.push('--setting-sources','user,project,local');
  // Each message written to the client is echoed back at the moment the model
  // reads it. That is how a message sent mid-turn is known to have joined the
  // turn, or to be waiting for one of its own.
  args.push('--replay-user-messages');
  args.push('--print','--verbose','--input-format','stream-json','--output-format','stream-json','--include-partial-messages','--permission-prompt-tool','stdio');
  return {args,resume,sessionId,model,effort};
}

// What currently occupies the context window. Claude Code documents this as the
// input tokens in the window including cache reads and writes, so the sum is
// taken from those official fields rather than inferred from message text.
function contextTokens(usage) {
  const parts=[usage?.input_tokens,usage?.cache_read_input_tokens,usage?.cache_creation_input_tokens];
  if(!parts.some(value=>Number.isSafeInteger(value)&&value>=0))return undefined;
  return parts.reduce((total,value)=>total+(Number.isSafeInteger(value)&&value>=0?value:0),0);
}

// The client publishes its models at initialize time, each with the effort
// levels that model actually supports. A model that states none genuinely has
// none, so no default ladder is substituted for it.
function modelEntries(value) {
  if(!Array.isArray(value))return undefined;
  return value.flatMap(entry=>{
    if(!entry||typeof entry!=='object'||typeof entry.value!=='string'||!entry.value)return [];
    return [{id:entry.value,name:typeof entry.displayName==='string'?entry.displayName:entry.value,
      ...(typeof entry.description==='string'?{description:entry.description}:{}),
      ...(entry.supportsEffort===true&&Array.isArray(entry.supportedEffortLevels)?{efforts:entry.supportedEffortLevels}:{})}];
  });
}

// The client reports commands as objects at initialize time and as plain names
// in its later init message. Both are reduced to names; nothing is added.
function commandNames(value) {
  if(!Array.isArray(value))return undefined;
  return value.map(entry=>typeof entry==='string'?entry:entry&&typeof entry==='object'?entry.name:undefined).filter(name=>typeof name==='string');
}

/** The client echoes `uuid` back when it reads a message; AgentDock's ids are UUIDs already. */
const UUID=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const wireId=id=>UUID.test(id)?id:randomUUID();
/** Whether a listed selection still describes the model the client resolved it to. */
export function keepsSelection(models,selected,reported){
  if(!selected||selected===reported||!models?.some(entry=>entry.id===selected))return selected===reported;
  if(selected==='default')return true;
  return reported.toLowerCase().includes(selected.replace(/\[.*\]$/,'').toLowerCase());
}
export class ClaudeChat extends ChatBase {
  async initialize() {
    this.launch=claudeLaunch(this.job);this.tools=new Map();
    this.port=new NativeProcess(this.job.program,this.launch.args,this.job.cwd,message=>this.notification(message),message=>this.fatal(message),()=>this.fatal('Claude Code exited.'),this.options);
    // This drives the user's installed native CLI, not SDK query() or an OAuth/token proxy.
    const initialized=await this.port.rpc('initialize',{promptSuggestions:false},true);
    // The client lists its own commands here, before any turn has run, so the
    // composer can offer them on a brand new session.
    this.announce(this.launch.resume??this.launch.sessionId,commandNames(initialized?.commands));
    this.models=modelEntries(initialized?.models);
    // Before the first turn the client does not say which model it is on. What
    // it does say is that `default` means "whatever this configuration
    // resolves to", so that is a truthful starting point — and only when the
    // client itself listed it. A launch flag is more specific, so it wins.
    this.model=this.launch.model??(this.models?.some(entry=>entry.id==='default')?'default':undefined);
    // The depth it was launched with is the one in force; saying nothing
    // left the chip reading "automatic" over a session running deeper.
    this.effort=this.launch.effort;
    this.settings(this.models,this.model,this.effort);
  }
  /**
   * Switch model in place. `set_model` is the client's own live control, so the
   * native context is kept: this is not an endpoint change and must never be
   * turned into one. An unknown name is refused by the client, not by us.
   */
  async selectModel(message) {
    if(this.active)throw Error('Wait for the current turn to finish before changing the model.');
    // A depth-only change keeps the current model; the client has no separate
    // control for it, and guessing a model here would switch the user's silently.
    const model=message.model??this.model;
    if(!model)throw Error('Claude Code has not reported which model this session uses yet. Send a message first, then adjust the thinking depth.');
    const effort=message.effort;
    await this.port.rpc('set_model',{model,...(effort?{effort}:{})},true);
    this.model=model;this.effort=effort??this.effort;
    this.settings(this.models,this.model,this.effort);
  }
  /**
   * Change how tools are approved, on the session already running.
   *
   * The client owns this: it accepts the change, applies it, and announces the
   * mode it ended up in through a status message. What comes back is what is
   * reported, so the interface shows the client's own state rather than the one
   * that was asked for -- they differ when the client declines.
   */
  get permissionModes(){return ['ask','plan','accept_edits','danger'];}
  async setPermissionMode(message) {
    if(this.active)throw Error('Wait for the current turn to finish before changing permissions.');
    const mode=CLAUDE_PERMISSION[message.mode];
    if(!mode)throw Error('Unsupported permission mode.');
    const result=await this.port.rpc('set_permission_mode',{mode},true);
    // The answer comes back in the client's own vocabulary, so it is translated
    // before it is reported. An answer this bridge cannot name is left to the
    // status message to settle rather than guessed at here.
    const applied=AGENTDOCK_PERMISSION[typeof result?.mode==='string'?result.mode:mode];
    if(applied)this.permissionMode=applied;
    this.settings(this.models,this.model,this.effort);
  }
  message(message) {
    const active=this.begin(message);if(!active)return;
    this.assistantId=undefined;this.finalAssistant=false;
    try{this.port.send({type:'user',uuid:wireId(message.id),session_id:this.nativeSessionId??'',message:{role:'user',content:message.content},parent_tool_use_id:null});}
    catch{this.error('Could not send the message to Claude Code.');this.finish('failed');}
  }
  /**
   * Steering needs nothing of Claude Code but the message: written while a
   * turn runs, the model reads it at its next step. If the turn ends first it
   * becomes a turn of its own, which the result handler below follows.
   */
  steer(message) {
    if(!this.active){this.message(message);return;}
    if(!this.acceptSteer(message))return;
    this.unconsumed??=new Set();this.unconsumed.add(message.id);
    try{this.port.send({type:'user',uuid:wireId(message.id),session_id:this.nativeSessionId??'',message:{role:'user',content:message.content},parent_tool_use_id:null});}
    catch{this.unconsumed.delete(message.id);this.error('Could not send the message to Claude Code.');}
  }
  /** A steered message the turn ended without reading runs next, under its own id. */
  continueSteered() {
    const next=this.unconsumed?.values().next().value;if(next===undefined)return;
    this.unconsumed.delete(next);this.assistantId=undefined;this.finalAssistant=false;
    this.active={id:next,interrupted:false};this.emit({type:'turn',id:next,status:'running'});
  }
  async interrupt() {
    const active=this.active;if(!active)return;active.interrupted=true;this.clearApprovals();this.unconsumed?.clear();
    try{await this.port.rpc('interrupt',{},true);}catch{if(this.active===active)this.error('Claude Code could not confirm turn interruption.');}
  }
  notification(message) {
    if(!message||typeof message!=='object')return;
    if(message.type==='control_request'){this.request(message);return;}
    if(message.type==='control_cancel_request'){this.resolveNative(String(message.request_id));return;}
    // The client reports the mode it is actually in, including after a change
    // it made itself. Following its word rather than the last request is what
    // keeps the interface from claiming a mode the client is not in.
    if(message.type==='system'&&typeof message.permissionMode==='string'){
      const mode=AGENTDOCK_PERMISSION[message.permissionMode];
      if(mode&&mode!==this.permissionMode){this.permissionMode=mode;this.settings(this.models,this.model,this.effort);}
    }
    if(message.type==='system'&&message.subtype==='init'){
      this.announce(message.session_id,commandNames(message.slash_commands));
      // The init message names the model actually in use. Knowing it is what
      // lets a depth-only change be applied in place, since set_model needs a
      // model and inventing one would silently switch the user off theirs.
      // It names the resolved id (claude-sonnet-5) even when the choice was a
      // list entry (sonnet, default). Keeping the entry that still describes
      // it is what keeps the chip on "Sonnet" instead of an unlisted id.
      if(typeof message.model==='string'&&message.model&&!keepsSelection(this.models,this.model,message.model))this.model=message.model;
      this.settings(this.models,this.model);
      return;
    }
    // The echo of a message the model just read. A steered one is no longer
    // waiting; one read with no turn open -- after an interrupt, say -- opens
    // one, so its reply is not dropped.
    if(message.type==='user'&&message.isReplay===true){
      const id=typeof message.uuid==='string'?message.uuid:undefined;
      if(id)this.unconsumed?.delete(id);
      if(id&&!this.active&&this.seen.has(id)){this.assistantId=undefined;this.finalAssistant=false;this.active={id,interrupted:false};this.emit({type:'turn',id,status:'running'});}
      return;
    }
    if(!this.active)return;
    // A subagent's output belongs to the tool call that started it. Emitting it
    // as a message would overwrite the main assistant reply, which is why this
    // was dropped; routing it to the parent card shows the work instead.
    if(typeof message.parent_tool_use_id==='string'&&message.parent_tool_use_id){this.subagent(message);return;}
    if(message.type==='stream_event'){
      const event=message.event??{};
      if(event.type==='message_start')this.assistantId=event.message?.id;
      else if(event.type==='content_block_delta'&&event.delta?.type==='text_delta')this.emit({type:'message',id:this.assistantId??`assistant-${this.active.id}`,role:'assistant',text:clip(event.delta.text),delta:true});
      else if(event.type==='content_block_start'&&event.content_block?.type==='tool_use')this.tool(event.content_block,'running');
      return;
    }
    if(message.type==='assistant'){
      const body=message.message??{},blocks=Array.isArray(body.content)?body.content:[];
      if(typeof body.id==='string')this.assistantId=body.id;
      const text=blocks.filter(block=>block.type==='text'&&typeof block.text==='string').map(block=>block.text).join('');
      if(text){this.finalAssistant=true;this.emit({type:'message',id:this.assistantId??`assistant-${this.active.id}`,role:'assistant',text:clip(text),delta:false});}
      for(const block of blocks)if(block.type==='tool_use')this.tool(block,'running');
      this.usage(body.usage,contextTokens(body.usage));return;
    }
    if(message.type==='user'&&Array.isArray(message.message?.content)){
      for(const result of message.message.content)if(result.type==='tool_result'&&typeof result.tool_use_id==='string'){
        const text=typeof result.content==='string'?result.content:Array.isArray(result.content)?result.content.filter(block=>block.type==='text').map(block=>block.text??'').join('\n'):'';
        this.emit({type:'tool',id:result.tool_use_id,name:this.tools.get(result.tool_use_id)??'Native tool',status:result.is_error?'failed':'completed',text:clip(text)});
      }return;
    }
    if(message.type==='tool_progress'&&typeof message.tool_use_id==='string'){
      this.emit({type:'tool',id:message.tool_use_id,name:clip(message.tool_name??'Native tool',256),status:'running'});return;
    }
    if(message.type==='result'){
      // Claude reports the window per model it actually used this turn. Only a
      // single agreed size is forwarded; mixed sizes stay unreported.
      const windows=new Set(Object.values(message.modelUsage??{}).map(entry=>entry?.contextWindow).filter(value=>Number.isSafeInteger(value)&&value>0));
      // The result totals are cumulative over every API call in the turn, so
      // deriving a context size from them counts the same prompt once per tool
      // round and reports several times the real occupancy. Only the last
      // assistant message describes the live context, so that value is kept and
      // just the window is taken from here.
      this.usage(message.usage,undefined,windows.size===1?[...windows][0]:undefined);
      // An empty result is not an answer; a blank bubble only looks like a bug.
      if(!this.finalAssistant&&typeof message.result==='string'&&message.result.trim())this.emit({type:'message',id:this.assistantId??`assistant-${this.active.id}`,role:'assistant',text:clip(message.result),delta:false});
      const failed=message.is_error||message.subtype!=='success';
      if(failed&&!this.active.interrupted){
        const diagnostic=JSON.stringify(message.errors??[]);
        this.error(/oauth|subscription.*(?:not|unsupported)|authentication.*(?:not|unsupported)/i.test(diagnostic)?'Claude Code does not support this authentication mode for this interface. Use its native client or an approved API configuration.':'Claude Code could not complete the turn. Check its native account, endpoint and permission settings.');
      }
      const interrupted=this.active.interrupted;
      this.finish(interrupted?'interrupted':failed?'failed':'completed');
      if(!interrupted)this.continueSteered();
    }
  }
  // Progress only: an activity line names what the subagent is doing and never
  // replaces the card's own text, which holds the tool's input and result.
  subagent(message) {
    const parent=message.parent_tool_use_id;
    if(!this.tools.has(parent))return; // A card we never announced has nothing to attach to.
    const lines=[];
    if(message.type==='assistant'){
      for(const block of message.message?.content??[]){
        if(block.type==='text'&&block.text?.trim())lines.push(block.text.trim());
        else if(block.type==='tool_use')lines.push(`\u2192 ${clip(block.name??'tool',120)}`);
      }
    }
    if(!lines.length)return;
    this.emit({type:'tool',id:parent,name:this.tools.get(parent),status:'running',activity:clip(lines.join('\n'),4096)});
  }

  tool(block,status) {
    if(typeof block.id!=='string')return;
    const name=clip(block.name??'Native tool',256);this.tools.set(block.id,name);if(this.tools.size>512)this.tools.delete(this.tools.keys().next().value);
    this.emit({type:'tool',id:block.id,name,status,text:clip(JSON.stringify(block.input??{}))});
  }
  reject(message,text='This Claude Code interaction is unsupported in AgentDock.') {
    this.port.send({type:'control_response',response:{subtype:'error',request_id:message.request_id,error:text}});this.error(text);
  }
  request(message) {
    const request=message.request??{},key=String(message.request_id);
    if(typeof message.request_id!=='string'||!message.request_id||message.request_id.length>256){this.fatal('Invalid native request identifier.');return;}
    if(request.subtype==='request_user_dialog'){
      // SDK protocol says undeclared dialog kinds must not be answered, including with error/cancel.
      this.error('This native dialog must be handled in Claude Code directly.');void this.interrupt();return;
    }
    if(request.subtype!=='can_use_tool'||!this.active){this.reject(message);return;}
    const input=request.input&&typeof request.input==='object'?request.input:{};
    if(Buffer.byteLength(JSON.stringify(input))>MAX_TEXT){this.reject(message,'Native approval is too large to display safely.');return;}
    if(request.tool_name==='AskUserQuestion'){
      const questions=Array.isArray(input.questions)?input.questions:[];
      if(!questions.length||questions.length>16||new Set(questions.map(q=>q.question)).size!==questions.length||questions.some(q=>typeof q.question!=='string'||(q.options!=null&&(!Array.isArray(q.options)||q.options.length>32||q.options.some(option=>typeof option?.label!=='string'))))){this.reject(message);return;}
      this.approval(key,{title:'Claude Code needs your input',text:'Answer the native client questions.',choices:['accept','decline','cancel'],questions:questions.map((q,index)=>({id:`question-${index}`,header:clip(q.header??'',128),question:clip(q.question),options:(q.options??[]).map(option=>({label:clip(option.label,256),description:clip(option.description??'',2048)})),multiSelect:!!q.multiSelect,isOther:true}))},(decision,answers)=>{
        if(decision==='accept'&&questions.some((q,index)=>!Array.isArray(answers[`question-${index}`])||!answers[`question-${index}`].length||(!q.multiSelect&&answers[`question-${index}`].length>1)))throw Error('Valid answers required');
        const updatedInput={...input,answers:Object.fromEntries(questions.map((q,index)=>[q.question,(answers[`question-${index}`]??[]).join(', ')]))};
        this.permissionResponse(message,decision,updatedInput);
      });return;
    }
    if(request.requires_user_interaction){this.reject(message,'This approval requires a native Claude Code dialog.');return;}
    const text=JSON.stringify({reason:request.decision_reason,input});
    if(Buffer.byteLength(text)>MAX_TEXT){this.reject(message,'Native approval is too large to display safely.');return;}
    this.approval(key,{title:clip(request.title??`Allow ${request.tool_name??'native tool'}?`,512),text:clip(text),choices:['accept','decline','cancel']},decision=>this.permissionResponse(message,decision,input));
  }
  permissionResponse(message,decision,input) {
    const result=decision==='accept'?{behavior:'allow',updatedInput:input,toolUseID:message.request.tool_use_id}:{behavior:'deny',message:'The user declined this tool request.',interrupt:decision==='cancel',toolUseID:message.request.tool_use_id};
    this.port.send({type:'control_response',response:{subtype:'success',request_id:message.request_id,response:result}});
    if(decision==='cancel'&&this.active)this.active.interrupted=true;
  }
}
