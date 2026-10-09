#!/usr/bin/env node
// Synthetic `pi --mode rpc` only. Never launches a shell, model or network request.
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';
const log=value=>appendFileSync(process.env.AGENTDOCK_CHAT_TEST_LOG,JSON.stringify(value)+'\n');
const send=message=>process.stdout.write(JSON.stringify(message)+'\n');
const respond=(command,data,extra={})=>send({id:command.id,type:'response',command:command.type,success:true,...(data===undefined?{}:{data}),...extra});
log({event:'args',args:process.argv.slice(2)});
let model={provider:'agentdock',id:'qwen-local',contextWindow:40000},holding=false,asked;
function reply(text,stopReason='stop'){
  send({type:'message_start',message:{role:'assistant'}});
  for(const delta of ['Hello ','🦊'])send({type:'message_update',assistantMessageEvent:{type:'text_delta',contentIndex:0,delta}});
  send({type:'tool_execution_start',toolCallId:'call-1',toolName:'bash',args:{command:'ls'}});
  send({type:'tool_execution_end',toolCallId:'call-1',toolName:'bash',result:{content:[{type:'text',text:'README.md'}]},isError:false});
  send({type:'message_end',message:{role:'assistant',content:[{type:'text',text}],usage:{input:12,output:4,cacheRead:30,cacheWrite:0},stopReason}});
  send({type:'agent_end',messages:[]});send({type:'agent_settled'});
}
for await(const line of createInterface({input:process.stdin})){
  const command=JSON.parse(line);log(command);
  if(command.type==='get_state')respond(command,{model,thinkingLevel:'off',sessionId:'pi-session-1',isStreaming:false});
  else if(command.type==='get_commands')respond(command,{commands:[{name:'skill:review',source:'skill'}]});
  else if(command.type==='get_available_models')respond(command,{models:[model,{provider:'kunlun',id:'zai-org/GLM-5.3',name:'GLM',reasoning:true}]});
  else if(command.type==='set_model'){model={provider:command.provider,id:command.modelId,contextWindow:128000};respond(command,model);}
  else if(command.type==='set_thinking_level'||command.type==='steer')respond(command);
  else if(command.type==='prompt'){
    respond(command);send({type:'agent_start'});
    if(command.message==='hold')holding=true;
    else if(command.message==='confirm'){asked='ui-1';send({type:'extension_ui_request',id:asked,method:'confirm',title:'Delete build output?',message:'rm -rf dist'});}
    else reply('Hello 🦊');
  }
  else if(command.type==='extension_ui_response'&&command.id===asked){asked=undefined;reply(command.confirmed?'Deleted':'Kept');}
  else if(command.type==='abort'){respond(command);if(holding){holding=false;send({type:'message_end',message:{role:'assistant',content:[],stopReason:'aborted'}});send({type:'agent_settled'});}}
  else send({id:command.id,type:'response',command:command.type,success:false,error:`Unknown command: ${command.type}`});
}
