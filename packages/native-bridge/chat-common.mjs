import { spawn } from 'node:child_process';
import { randomUUID, createHash } from 'node:crypto';
import { StringDecoder } from 'node:string_decoder';

export const MAX_INPUT_LINE = 512 * 1024;
export const MAX_NATIVE_LINE = 4 * 1024 * 1024;
export const MAX_TEXT = 64 * 1024;
export const MAX_EVENT_LINE = 192 * 1024;
export const nativeId = value => typeof value === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(value);
export function clip(value, limit = MAX_TEXT) {
  if (typeof value !== 'string') return '';
  const cleaned = value.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '').replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '');
  const bytes = Buffer.from(cleaned);
  return bytes.length <= limit ? cleaned : bytes.subarray(0, limit).toString('utf8') + '\n[truncated]';
}
export function redactEvents(environment, output) {
  const secrets = Object.entries(environment).filter(([key, value]) => /TOKEN|SECRET|PASSWORD|PRIVATE_KEY|API_KEY|AUTHORIZATION/i.test(key) && typeof value === 'string' && value.length >= 4).map(([, value]) => value).sort((a,b) => b.length-a.length);
  return event => {
    const scrub = value => typeof value === 'string' ? secrets.reduce((text, secret) => text.split(secret).join('[redacted]'), value) : Array.isArray(value) ? value.map(scrub) : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).map(([key,item]) => [key,scrub(item)])) : value;
    output(scrub(event));
  };
}

/** Bounded UTF-8 JSONL, including a fixed deadline for incomplete lines. */
export function jsonLines(readable, onMessage, onFailure, { maxLine = MAX_NATIVE_LINE, maxTotal = 128 * 1024 * 1024, lineTimeout = 10_000 } = {}) {
  const decoder = new StringDecoder('utf8'); let buffer = '', total = 0, timer, stopped = false;
  const clear = () => { clearTimeout(timer); timer = undefined; };
  const fail = message => { if (!stopped) { stopped = true; clear(); onFailure(message); } };
  const data = chunk => {
    if (stopped) return;
    total += chunk.length; if (total > maxTotal) { fail('Structured stream exceeded its size limit.'); return; }
    buffer += decoder.write(chunk);
    let index;
    while ((index = buffer.indexOf('\n')) !== -1) {
      clear(); const line = buffer.slice(0,index); buffer = buffer.slice(index+1);
      if (Buffer.byteLength(line) > maxLine) { fail('Structured message exceeded its size limit.'); return; }
      if (!line.trim()) continue;
      try { onMessage(JSON.parse(line)); } catch { fail('Invalid structured message received.'); return; }
      if (stopped) return;
    }
    if (Buffer.byteLength(buffer) > maxLine) { fail('Structured message exceeded its size limit.'); return; }
    if (buffer && !timer) timer = setTimeout(() => fail('Incomplete structured message timed out.'), lineTimeout);
  };
  const end = () => { buffer += decoder.end(); if (buffer.trim() && !stopped) fail('Structured stream ended mid-message.'); clear(); };
  readable.on('data',data); readable.on('end',end);
  return () => { stopped = true; clear(); readable.off('data',data); readable.off('end',end); };
}

export class NativeProcess {
  constructor(program, args, cwd, onMessage, onFailure, onExit, { spawnProcess = spawn, rpcTimeout = 30_000 } = {}) {
    this.pending = new Map(); this.sequence = 0; this.closed = false; this.rpcTimeout = rpcTimeout;
    this.failure = message => { if (!this.closed) onFailure(message); };
    this.child = spawnProcess(program,args,{ cwd, env: process.env, stdio:['pipe','pipe','pipe'], shell:false });
    this.stopLines = jsonLines(this.child.stdout, message => {
      const responseId = message?.type === 'control_response' ? message.response?.request_id : !message?.method ? message?.id : undefined;
      const pending = this.pending.get(responseId);
      if (pending) {
        this.pending.delete(responseId); clearTimeout(pending.timer);
        if (message.error || message.response?.subtype === 'error') {
          // The client says exactly why it refused. Dropping that left the user
          // with a generic message and nothing to act on, so its own wording is
          // carried through — clipped, and already redacted on the way out.
          const stated = message.response?.error ?? message.error?.message ?? message.error;
          const detail = typeof stated === 'string' && stated.trim() ? clip(stated.trim(), 400) : undefined;
          pending.reject(new Error(detail
            ? `Claude Code refused: ${detail}`
            : 'Native client rejected a control request. Check client settings and version.'));
        }
        else pending.resolve(message.type === 'control_response' ? message.response.response ?? {} : message.result);
      } else onMessage(message);
    }, this.failure);
    let diagnosticBytes = 0;
    this.child.stderr.on('data',chunk => { diagnosticBytes += chunk.length; if (diagnosticBytes > 16 * 1024 * 1024) this.failure('Native diagnostic output exceeded its size limit.'); });
    this.child.stdin.on('error',() => this.failure('Native client input closed.'));
    this.child.once('error',() => this.failure('Could not launch the configured native client.'));
    this.child.once('exit',() => {
      this.stopLines(); this.rejectPending(); if (!this.closed) onExit();
    });
  }
  rejectPending() { for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(new Error('Native client closed.')); } this.pending.clear(); }
  send(message) {
    if (this.closed || !this.child.stdin.writable) throw new Error('Native client input is unavailable.');
    const line = JSON.stringify(message) + '\n';
    if (Buffer.byteLength(line) > MAX_NATIVE_LINE || this.child.stdin.writableLength > MAX_NATIVE_LINE) throw new Error('Native input exceeded its buffer limit.');
    this.child.stdin.write(line);
  }
  /**
   * `optional` is for a capability probe: a client that does not implement the
   * method may simply never answer, and that must not look like a broken
   * connection. Such a call fails quietly and changes nothing.
   */
  rpc(method, params, claude = false, optional = false) {
    const id = `agentdock-${++this.sequence}`;
    return new Promise((resolve,reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);reject(new Error('Native control request timed out.'));
        // A timed-out start may have executed. Do not reopen the send gate and guess/replay it.
        if(!optional&&method!=='interrupt'&&method!=='turn/interrupt')this.failure('Native control request timed out; the structured connection was stopped.');
      },optional?Math.min(this.rpcTimeout,4000):this.rpcTimeout);
      this.pending.set(id,{resolve,reject,timer});
      try { this.send(claude ? {type:'control_request',request_id:id,request:{subtype:method,...params}} : {id,method,params}); }
      catch(error) { clearTimeout(timer);this.pending.delete(id);reject(error); }
    });
  }
  async close() {
    if (this.closed) return;
    this.closed = true; this.stopLines(); this.rejectPending();
    this.child.stdin.end(); this.child.kill('SIGTERM');
    await new Promise(resolve => {
      if (this.child.exitCode !== null || this.child.signalCode) { resolve();return; }
      const timer = setTimeout(() => { this.child.kill('SIGKILL');resolve(); },1500);
      this.child.once('exit',() => {clearTimeout(timer);resolve();});
    });
  }
}

export class ChatBase {
  constructor(job, emit, options = {}) { this.job=job;this.emit=emit;this.options=options;this.seen=new Map();this.approvals=new Map();this.ready=false;this.active=undefined;this.closed=false; }
  error(message) { this.emit({type:'error',message:clip(message,2048)}); }
  begin(message) {
    if (!this.ready || this.closed) { this.error('The native client is not ready.');return; }
    const digest=createHash('sha256').update(message.content).digest('hex');
    if (this.seen.has(message.id)) { if(this.seen.get(message.id)!==digest)this.error('Message ID was already used for different content.');return; }
    if(this.active){this.error('A turn is already running. Interrupt it or wait before sending another message.');return;}
    if(this.seen.size>=4096){this.error('This bridge reached its message limit. Reopen the saved native session.');this.emit({type:'turn',id:message.id,status:'failed'});return;}
    this.seen.set(message.id,digest);this.active={id:message.id,interrupted:false};
    this.emit({type:'message',id:message.id,role:'user',text:clip(message.content),delta:false});
    this.emit({type:'turn',id:message.id,status:'running'});return this.active;
  }
  finish(status) {
    if(!this.active)return;
    const id=this.active.id;this.active=undefined;
    this.clearApprovals(false);this.emit({type:'turn',id,status});
  }
  // `commands` are the slash commands the native client itself advertised on
  // startup. They are forwarded verbatim so the composer can offer exactly what
  // that client accepts, never a list AgentDock invented.
  announce(id,commands) {
    // A client that hands back a different session has started a fresh context
    // of its own — /clear does exactly this. The transcript above it describes
    // a context the client no longer has, so it is not kept as if it did.
    if(nativeId(id)&&this.nativeSessionId&&id!==this.nativeSessionId)this.emit({type:'cleared'});
    if(nativeId(id))this.nativeSessionId=id;
    this.ready=true;
    const names=Array.isArray(commands)
      ? [...new Set(commands.filter(name=>typeof name==='string'&&/^[A-Za-z0-9][\w:-]{0,63}$/.test(name)))].slice(0,400)
      : undefined;
    this.emit({type:'ready',...(this.nativeSessionId?{native_session_id:this.nativeSessionId}:{}),...(names?.length?{commands:names}:{})});
  }
  /**
   * What the client says it can run right now: the models it offers and which
   * one is selected. This is deliberately not a `configuration` event — that one
   * marks a context boundary, and selecting a model must not start a new one.
   * Entries are forwarded from the client's own list; nothing is added.
   */
  settings(models, model, effort) {
    const list=Array.isArray(models)
      ? models.flatMap(entry=>{
          if(!entry||typeof entry!=='object'||typeof entry.id!=='string'||!entry.id||entry.id.length>128)return [];
          // Only levels both clients share. An unfamiliar one drops itself
          // rather than taking the whole list down with it downstream.
          const EFFORTS=['low','medium','high','xhigh','max'];
          const efforts=Array.isArray(entry.efforts)?entry.efforts.filter(value=>EFFORTS.includes(value)).slice(0,16):undefined;
          return [{id:entry.id,name:clip(typeof entry.name==='string'&&entry.name?entry.name:entry.id,128),
            ...(typeof entry.description==='string'&&entry.description?{description:clip(entry.description,256)}:{}),
            // The row the configuration resolves to. A session that has not
            // started yet has no model of its own, and this is what it shows
            // instead of electing the first row or offering no depth at all.
            ...(entry.isDefault===true?{isDefault:true}:{}),
            ...(efforts?.length?{efforts}:{})}];
        }).slice(0,64)
      : undefined;
    const selected=typeof model==='string'&&model&&model.length<=128?model:undefined;
    const depth=typeof effort==='string'&&/^[a-z]{1,16}$/.test(effort)?effort:undefined;
    if(!list?.length&&!selected&&!depth)return;
    this.emit({type:'settings',...(selected?{model:selected}:{}),...(depth?{effort:depth}:{}),...(list?.length?{models:list}:{})});
  }
  approval(key, description, answer) {
    if(this.approvals.has(key))return;
    if(this.approvals.size>=32){answer(description.choices.includes('cancel')?'cancel':'decline',{});this.error('Too many pending native approvals.');return;}
    const id=randomUUID();
    const timer=setTimeout(() => {this.resolveApproval(key,description.choices.includes('cancel')?'cancel':'decline',{});this.error('Native approval timed out and was cancelled.');},this.options.approvalTimeout??600_000);
    this.approvals.set(key,{id,answer,timer,choices:description.choices});this.emit({type:'approval',id,...description});
  }
  resolveApproval(key,decision,answers) {
    const pending=this.approvals.get(key);if(!pending)return;
    try { pending.answer(decision,answers); } catch { this.error('Invalid or unavailable native approval response.');return; }
    if(this.approvals.get(key)!==pending)return;
    this.approvals.delete(key);clearTimeout(pending.timer);
    this.emit({type:'approval_resolved',id:pending.id});
  }
  answer(message) {
    const entry=[...this.approvals].find(([,pending])=>pending.id===message.request_id);
    if(!entry){this.error('This approval is no longer pending.');return;}
    if(!entry[1].choices.includes(message.decision)){this.error('This decision is not offered by the native request.');return;}
    this.resolveApproval(entry[0],message.decision,message.answers??{});
  }
  resolveNative(key) { const pending=this.approvals.get(key);if(pending){clearTimeout(pending.timer);this.approvals.delete(key);this.emit({type:'approval_resolved',id:pending.id});} }
  clearApprovals(sendCancellation=true) { for(const key of [...this.approvals.keys()])sendCancellation?this.resolveApproval(key,this.approvals.get(key).choices.includes('cancel')?'cancel':'decline',{}):this.resolveNative(key); }
  async shutdown() { if(this.closed)return;this.closed=true;this.clearApprovals();this.finish('interrupted');await this.port?.close(); }
  fatal(message) { if(this.closed)return;this.error(message);this.finish('failed');void this.shutdown().finally(()=>this.options.onExit?.()); }
  // `window` is the model's own context size as the client reported it. It is
  // only ever forwarded, never assumed, so a share of context can be shown when
  // the client states the limit and is simply omitted when it does not.
  usage(usage,context,window) {
    const values={input_tokens:usage?.inputTokens??usage?.input_tokens,output_tokens:usage?.outputTokens??usage?.output_tokens,context_tokens:context,context_window:window};
    const filtered=Object.fromEntries(Object.entries(values).filter(([key,value])=>Number.isSafeInteger(value)&&(key==='context_window'?value>0:value>=0)));
    if(Object.keys(filtered).length)this.emit({type:'usage',...filtered});
  }
}
