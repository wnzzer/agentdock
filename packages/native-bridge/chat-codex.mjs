import { resolve } from 'node:path';
import { ChatBase, NativeProcess, clip, nativeId, MAX_TEXT } from './chat-common.mjs';

export function codexLaunch(job) {
  const args=['app-server'],thread={cwd:job.cwd};let resume=job.resume_id;
  for(let i=0;i<job.args.length;i++) {
    const arg=job.args[i],equal=arg.indexOf('='),flag=equal>0?arg.slice(0,equal):arg;
    const value=()=>{const result=equal>0?arg.slice(equal+1):job.args[++i];if(typeof result!=='string'||!result)throw Error('Missing Codex launch option value.');return result;};
    if(flag==='-c'||flag==='--config'){args.push('-c',value());}
    else if(flag==='--model'||flag==='-m')thread.model=value();
    else if(flag==='--ask-for-approval'||flag==='-a')args.push('-c','approval_policy='+JSON.stringify(value()));
    else if(flag==='--sandbox'||flag==='-s')args.push('-c','sandbox_mode='+JSON.stringify(value()));
    else if(flag==='--enable'||flag==='--disable')args.push(flag,value());
    else if(flag==='--cd'||flag==='-C'){if(resolve(value())!==resolve(job.cwd))throw Error('Codex launch directory does not match the workspace.');}
    else if(flag==='resume'||flag==='--resume'){const id=value();if(!nativeId(id)||(resume&&resume!==id))throw Error('Invalid or conflicting Codex resume ID.');resume=id;}
    else if(flag==='--no-alt-screen'||flag==='app-server')continue;
    else throw Error('Unsupported Codex launch option in structured mode.');
  }
  return {args,thread,resume};
}
// Codex states the effort levels each model supports; a model that lists none
// is offered without a depth control rather than with an invented ladder.
function codexModels(result) {
  // app-server answers model/list with `data`; `models` is accepted too so a
  // build that renames it still works. Reading only `models` is what left
  // Codex sessions with no model picker and no depth control at all.
  const rows=Array.isArray(result?.data)?result.data
    :Array.isArray(result?.models)?result.models
    :Array.isArray(result)?result:undefined;
  if(!rows)return undefined;
  return rows.flatMap(row=>{
    const id=row&&typeof row==='object'?row.id??row.model??row.name:undefined;
    if(typeof id!=='string'||!id)return [];
    const efforts=Array.isArray(row.supportedReasoningEfforts)
      ? row.supportedReasoningEfforts.map(level=>typeof level==='string'?level:level?.reasoningEffort).filter(level=>typeof level==='string')
      : undefined;
    if(row.hidden===true)return [];
    return [{id,name:typeof row.displayName==='string'?row.displayName:id,...(row.isDefault===true?{isDefault:true}:{}),
      ...(typeof row.description==='string'?{description:row.description}:{}),
      ...(efforts?.length?{efforts}:{})}];
  });
}
const requestKey=id=>typeof id+':'+String(id);
const itemStatus=item=>['failed','declined','cancelled'].includes(item.status)||item.success===false?'failed':item.status==='inProgress'?'running':'completed';

/**
 * How AgentDock's modes reach Codex. It approves per command rather than by
 * switching review modes, so only the two ends of the range translate: ask, and
 * the policy that stops asking. `danger` also drops the sandbox, because an
 * unattended run blocked by the sandbox instead of by a prompt is the same
 * interruption under a different name.
 */
const CODEX_APPROVAL={
  ask:{approvalPolicy:'on-request'},
  danger:{approvalPolicy:'never',sandboxPolicy:{type:'dangerFullAccess'}},
};

export class CodexChat extends ChatBase {
  async initialize() {
    this.launch=codexLaunch(this.job);this.tools=new Map();
    this.port=new NativeProcess(this.job.program,this.launch.args,this.job.cwd,message=>this.notification(message),message=>this.fatal(message),()=>this.fatal('Codex app-server exited.'),this.options);
    await this.port.rpc('initialize',{clientInfo:{name:'agentdock',title:'AgentDock',version:'0.1.0'}});
    this.port.send({method:'initialized',params:{}});this.announce();
    this.settings(undefined,this.launch.thread.model);
    // Asking for models must not hold up the session. A Codex build without
    // model/list simply offers no picker rather than a list AgentDock guessed.
    void this.port.rpc('model/list',{limit:100,includeHidden:false},false,true)
      .then(result=>{
        this.models=codexModels(result);
        // Codex runs its own default when a turn names no model, and it says
        // which that is. Reporting it is what gives a session that never chose
        // a model its depth control, which reads from the model in use.
        this.defaultModel=this.models?.find(entry=>entry.isDefault)?.id;
        this.settings(this.models,this.launch.thread.model??this.defaultModel,this.effort);
      })
      .catch(()=>{});
  }
  /**
   * Codex takes the model per turn, so the next `turn/start` carries the new
   * one and the thread — with all of its context — stays exactly as it is.
   */
  // Codex approves per command rather than per mode, so it has no plan or
  // accept-edits equivalent to offer. `danger` is the policy that stops asking.
  get permissionModes(){return ['ask','danger'];}
  setPermissionMode(message) {
    if(this.active)throw Error('Wait for the current turn to finish before changing permissions.');
    const policy=CODEX_APPROVAL[message.mode];
    if(!policy)throw Error('Codex approves each command rather than switching between review modes.');
    this.approvalPolicy=policy.approvalPolicy;
    this.sandboxPolicy=policy.sandboxPolicy;
    this.permissionMode=message.mode;
    this.settings(this.models,this.launch.thread.model??this.defaultModel,this.effort);
  }
  selectModel(message) {
    if(this.active)throw Error('Wait for the current turn to finish before changing the model.');
    if(message.model)this.launch.thread.model=message.model;
    if(message.effort)this.effort=message.effort;
    this.settings(this.models,this.launch.thread.model??this.defaultModel,this.effort);
  }
  async message(message) {
    const active=this.begin(message);if(!active)return;
    await this.run(active,message.content);
  }
  async run(active,content) {
    try {
      if(!this.nativeSessionId){
        const result=await this.port.rpc(this.launch.resume?'thread/resume':'thread/start',{...this.launch.thread,...(this.launch.resume?{threadId:this.launch.resume}:{})});
        if(!nativeId(result?.thread?.id))throw Error('Codex did not return a usable native thread ID.');
        this.announce(result.thread.id);
      }
      if(this.active!==active)return;
      if(active.interrupted){this.finish('interrupted');return;}
      // Codex takes both per turn, so a change applies to the next one without
      // touching the thread or its context.
      const result=await this.port.rpc('turn/start',{threadId:this.nativeSessionId,input:[{type:'text',text:content}],...(this.launch.thread.model?{model:this.launch.thread.model}:{}),...(this.effort?{effort:this.effort}:{}),...(this.approvalPolicy?{approvalPolicy:this.approvalPolicy}:{}),...(this.sandboxPolicy?{sandboxPolicy:this.sandboxPolicy}:{})});
      if(this.active===active){
        active.nativeTurn=result?.turn?.id;
        if(active.interrupted)await this.sendInterrupt(active);
        // Steering written before the turn had an id waited for it.
        else for(const early of active.early.splice(0))await this.sendSteer(active,early);
      }
    }catch(error){if(this.active===active){this.error(error.message);this.finish(active.interrupted?'interrupted':'failed');}}
  }
  /**
   * Codex steers through `turn/steer`, bound to the turn it was written for:
   * `expectedTurnId` makes it fail rather than land in a later turn. A turn
   * that cannot take it -- /review, a manual /compact -- or one that ended in
   * the meantime leaves it for the next turn, which starts on its own.
   */
  async steer(message) {
    const active=this.active;
    if(!active){await this.message(message);return;}
    if(!this.acceptSteer(message))return;
    if(!active.nativeTurn){active.early.push(message);return;}
    await this.sendSteer(active,message);
  }
  async sendSteer(active,message) {
    try{await this.port.rpc('turn/steer',{threadId:this.nativeSessionId,expectedTurnId:active.nativeTurn,input:[{type:'text',text:message.content}]});}
    catch{if(active.interrupted)return;this.deferred.push(message);if(this.active!==active)this.continueDeferred();}
  }
  /** Steering a turn would not take runs as the next turn, all of it at once. */
  continueDeferred() {
    if(this.active||!this.deferred.length)return;
    const list=this.deferred.splice(0),id=list[0].id;
    this.active={id,interrupted:false,early:[]};this.emit({type:'turn',id,status:'running'});
    void this.run(this.active,list.map(item=>item.content).join('\n\n'));
  }
  async sendInterrupt(active) {
    if(!active.nativeTurn||active.interruptSent)return;
    active.interruptSent=true;
    try{await this.port.rpc('turn/interrupt',{threadId:this.nativeSessionId,turnId:active.nativeTurn});}
    catch{if(this.active===active){active.interruptSent=false;this.error('Codex could not confirm turn interruption.');}}
  }
  interrupt() { if(!this.active)return;this.active.interrupted=true;this.deferred.length=0;this.clearApprovals();void this.sendInterrupt(this.active); }
  notification(message) {
    if(!message||typeof message!=='object')return;
    if(message.method&&message.id!==undefined){this.request(message);return;}
    const p=message.params??{},method=message.method;
    if(p.threadId&&this.nativeSessionId&&p.threadId!==this.nativeSessionId)return;
    if(method==='serverRequest/resolved'){this.resolveNative(requestKey(p.requestId));return;}
    if(method==='thread/tokenUsage/updated'){const usage=p.tokenUsage?.last??p.tokenUsage?.total;this.usage(usage,p.tokenUsage?.last?.totalTokens,p.tokenUsage?.modelContextWindow);return;}
    if(method==='error'){this.error('Codex reported a native turn error. Check its account, endpoint and permission settings.');return;}
    if(!this.active)return;
    if(p.turnId&&this.active.nativeTurn&&p.turnId!==this.active.nativeTurn)return;
    if(method==='turn/started'){this.active.nativeTurn=p.turn?.id;if(this.active.interrupted)void this.sendInterrupt(this.active);return;}
    if(method==='turn/completed'){const status=p.turn?.status==='interrupted'?'interrupted':p.turn?.status==='failed'?'failed':'completed';this.finish(status);if(status==='interrupted')this.deferred.length=0;else this.continueDeferred();return;}
    if(method==='item/agentMessage/delta'||method==='item/plan/delta'){
      if(typeof p.itemId==='string'&&typeof p.delta==='string')this.emit({type:'message',id:p.itemId,role:'assistant',text:clip(p.delta),delta:true});return;
    }
    if(method==='item/commandExecution/outputDelta'){
      if(typeof p.itemId!=='string')return;
      const previous=this.tools.get(p.itemId)??{name:'Command',text:''};previous.text=clip(previous.text+(typeof p.delta==='string'?p.delta:''));this.tools.set(p.itemId,previous);
      this.emit({type:'tool',id:String(p.itemId),name:previous.name,status:'running',text:previous.text});return;
    }
    if(method!=='item/started'&&method!=='item/completed')return;
    const item=p.item;if(!item||typeof item.id!=='string')return;
    if(item.type==='agentMessage'||item.type==='plan'){
      if(typeof item.text==='string')this.emit({type:'message',id:item.id,role:'assistant',text:clip(item.text),delta:false});return;
    }
    if(item.type==='userMessage'||item.type==='reasoning')return;
    let name=item.tool||item.type||'Native tool',text='';
    if(item.type==='commandExecution'){name='Command';text=[item.command,item.aggregatedOutput].filter(value=>typeof value==='string').join('\n');}
    else if(item.type==='fileChange'){name='File changes';text=(item.changes??[]).map(change=>`${change.path??''}\n${change.diff??''}`).join('\n');}
    else if(item.type==='mcpToolCall'){name=[item.server,item.tool].filter(Boolean).join(' / ');text=JSON.stringify(item.arguments??{});}
    else text=typeof item.query==='string'?item.query:typeof item.path==='string'?item.path:'';
    text=clip(text);this.tools.set(item.id,{name:clip(name,256),text});
    if(this.tools.size>512)this.tools.delete(this.tools.keys().next().value);
    this.emit({type:'tool',id:item.id,name:clip(name,256),status:method==='item/started'?'running':itemStatus(item),text});
  }
  reject(message,reason='This native request is not supported by AgentDock.') {
    this.port.send({id:message.id,error:{code:-32601,message:reason}});this.error(reason);
  }
  request(message) {
    const p=message.params??{},method=message.method,key=requestKey(message.id);
    if(!((typeof message.id==='string'&&message.id.length>0&&message.id.length<=256)||(typeof message.id==='number'&&Number.isSafeInteger(message.id)))){this.fatal('Invalid native request identifier.');return;}
    if(!this.active||(p.threadId&&p.threadId!==this.nativeSessionId)){this.reject(message,'Native request does not belong to the active turn.');return;}
    if(method==='item/commandExecution/requestApproval'||method==='item/fileChange/requestApproval'){
      const text=JSON.stringify({reason:p.reason,command:p.command,cwd:p.cwd,grantRoot:p.grantRoot,network:p.networkApprovalContext,additionalPermissions:p.additionalPermissions});
      if(Buffer.byteLength(text)>MAX_TEXT){this.reject(message,'Native approval is too large to display safely.');return;}
      const supported=['accept','decline','cancel'];
      const choices=Array.isArray(p.availableDecisions)?supported.filter(decision=>p.availableDecisions.includes(decision)):supported;
      if(!choices.includes('decline')&&!choices.includes('cancel')){this.reject(message);return;}
      this.approval(key,{title:method.includes('commandExecution')?'Approve native command':'Approve native file changes',text:clip(text),choices},decision=>this.port.send({id:message.id,result:{decision}}));return;
    }
    if(method==='item/tool/requestUserInput'){
      const questions=Array.isArray(p.questions)?p.questions:[];
      if(!questions.length||questions.length>16||Buffer.byteLength(JSON.stringify(questions))>MAX_TEXT||new Set(questions.map(q=>q.id)).size!==questions.length||questions.some(q=>typeof q.id!=='string'||q.id.length>128||typeof q.question!=='string'||(q.options!=null&&(!Array.isArray(q.options)||q.options.length>32||q.options.some(option=>typeof option?.label!=='string'))))){this.reject(message);return;}
      this.approval(key,{title:'Native client needs your input',text:'Answer the native client questions.',choices:['accept','decline','cancel'],questions:questions.map(q=>({id:q.id,header:clip(q.header??'',128),question:clip(q.question),options:(q.options??[]).map(o=>({label:clip(o.label,256),description:clip(o.description??'',2048)})),isSecret:!!q.isSecret,isOther:!!q.isOther,multiSelect:!!q.multiSelect}))},(decision,answers)=>{
        if(decision==='accept'&&questions.some(q=>!Array.isArray(answers[q.id])||!answers[q.id].length||(!q.multiSelect&&answers[q.id].length>1)||(!q.isOther&&!q.isSecret&&q.options?.length&&answers[q.id].some(answer=>!q.options.some(option=>option.label===answer)))))throw Error('Valid answers required');
        const values=decision==='accept'?Object.fromEntries(questions.map(q=>[q.id,{answers:answers[q.id]}])):{};
        this.port.send({id:message.id,result:{answers:values}});if(decision==='cancel'&&this.active){this.active.interrupted=true;void this.sendInterrupt(this.active);}
      });return;
    }
    if(method==='item/permissions/requestApproval'){
      if(!p.permissions||typeof p.permissions!=='object'||Buffer.byteLength(JSON.stringify(p.permissions))>MAX_TEXT){this.reject(message);return;}
      this.approval(key,{title:'Approve native permission request',text:clip(JSON.stringify({reason:p.reason,permissions:p.permissions})),choices:['accept','decline','cancel']},decision=>{
        this.port.send({id:message.id,result:{permissions:decision==='accept'?p.permissions:{},scope:'turn'}});
        if(decision==='cancel'&&this.active){this.active.interrupted=true;void this.sendInterrupt(this.active);}
      });return;
    }
    this.reject(message);
  }
}
