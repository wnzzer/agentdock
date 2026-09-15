import { isAbsolute, resolve } from 'node:path';
import { stat } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { jsonLines, MAX_INPUT_LINE, MAX_EVENT_LINE, nativeId, redactEvents } from './chat-common.mjs';
import { CodexChat } from './chat-codex.mjs';
import { ClaudeChat } from './chat-claude.mjs';

export function validateChatInput(message, first=false) {
  if(!message||typeof message!=='object'||Array.isArray(message))throw Error('Expected a structured chat object.');
  if(first){
    if(message.type!=='init'||!['codex','claude_code'].includes(message.provider)||typeof message.program!=='string'||!message.program||message.program.includes('\0')||typeof message.cwd!=='string'||!isAbsolute(message.cwd)||!Array.isArray(message.args)||message.args.length>512||message.args.some(arg=>typeof arg!=='string'||arg.includes('\0'))||Buffer.byteLength(JSON.stringify(message.args))>64*1024||(message.resume_id!==undefined&&!nativeId(message.resume_id)))throw Error('Invalid native chat initialization.');
    return message;
  }
  if(message.type==='message'){
    if(typeof message.id!=='string'||!message.id||message.id.length>128||/[\u0000-\u001f]/.test(message.id)||typeof message.content!=='string'||!message.content.trim()||message.content.includes('\0')||Buffer.byteLength(message.content)>256*1024)throw Error('Invalid or oversized chat message.');
  }else if(message.type==='approval'){
    if(typeof message.request_id!=='string'||message.request_id.length>256||!['accept','decline','cancel'].includes(message.decision))throw Error('Invalid approval response.');
    if(message.answers!==undefined&&(!message.answers||typeof message.answers!=='object'||Array.isArray(message.answers)||Object.entries(message.answers).length>16||Object.entries(message.answers).some(([key,values])=>key.length>256||!Array.isArray(values)||values.length>32||values.some(value=>typeof value!=='string'||Buffer.byteLength(value)>8192))))throw Error('Invalid native question answers.');
  }else if(message.type==='model'){
    // The client validates the name itself and rejects one it cannot serve, so
    // this only bounds the shape: no list of model names is invented here.
    if(typeof message.model!=='string'||!message.model.trim()||message.model.length>128||/[\u0000-\u001f]/.test(message.model))throw Error('Invalid model selection.');
    if(message.effort!==undefined&&(typeof message.effort!=='string'||!/^[a-z]{1,16}$/.test(message.effort)))throw Error('Invalid reasoning effort.');
  }else if(!['interrupt','shutdown'].includes(message.type))throw Error('Unsupported chat control message.');
  return message;
}

export function runChatBridge(input=process.stdin,output=process.stdout,options={}) {
  let runtime,initialized=false,initializing=false,stopped=false,finished=false,stopLines;
  const emit=redactEvents(process.env,event=>{
    if(finished)return;
    if(output.writableLength>4*1024*1024){void shutdown();return;}
    const line=JSON.stringify(event)+'\n';
    if(Buffer.byteLength(line)>MAX_EVENT_LINE){fatal('Structured event exceeded its wire size limit.');return;}
    output.write(line);
  });
  let resolveDone;const done=new Promise(resolve=>{resolveDone=resolve;});
  const finish=()=>{if(finished)return;clearTimeout(initTimer);stopLines?.();emit({type:'exit'});finished=true;input.off('end',onEnd);input.off('error',onInputError);output.off('error',onOutputError);input.pause();resolveDone();};
  async function shutdown(){if(stopped)return;stopped=true;clearTimeout(initTimer);await runtime?.shutdown();finish();}
  const fatal=message=>{emit({type:'error',message});void shutdown();};
  const onEnd=()=>void shutdown(),onInputError=()=>fatal('Chat control input closed.'),onOutputError=()=>void shutdown();
  const initTimer=setTimeout(()=>fatal('Chat initialization input timed out.'),options.initTimeout??15_000);
  stopLines=jsonLines(input,message=>{
    void(async()=>{
      try{
        if(stopped)return;
        if(message?.type==='shutdown'){await shutdown();return;}
        if(initializing){emit({type:'error',message:'Chat initialization is still in progress.'});return;}
        if(!initialized){
          validateChatInput(message,true);initializing=true;clearTimeout(initTimer);
          if(!(await stat(message.cwd)).isDirectory())throw Error('Chat workspace is not a directory.');
          if(stopped)return;
          const Type=message.provider==='codex'?CodexChat:ClaudeChat;
          runtime=new Type(message,emit,{...options,onExit:finish});
          await runtime.initialize();initialized=true;initializing=false;
        }else{
          validateChatInput(message);
          if(message.type==='message')await runtime.message(message);
          else if(message.type==='approval')runtime.answer(message);
          else if(message.type==='interrupt')await runtime.interrupt();
          else if(message.type==='model')await runtime.selectModel(message);
          else await shutdown();
        }
      }catch(error){emit({type:'error',message:error instanceof Error?error.message:'Native chat operation failed.'});if(!initialized){void shutdown();}}
    })();
  },fatal,{maxLine:MAX_INPUT_LINE,maxTotal:64*1024*1024,lineTimeout:options.lineTimeout??10_000});
  input.on('end',onEnd);input.on('error',onInputError);output.on('error',onOutputError);
  return {done,shutdown};
}

if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const bridge=runChatBridge();const stop=()=>void bridge.shutdown();
  process.once('SIGTERM',stop);process.once('SIGINT',stop);
  await bridge.done;process.removeListener('SIGTERM',stop);process.removeListener('SIGINT',stop);
}
