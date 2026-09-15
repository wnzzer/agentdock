#!/usr/bin/env node
// Test-only Rust -> real chat bridge -> synthetic CLI adapter. Never use as a production bridge.
import { Transform } from 'node:stream';
import { isAbsolute, join } from 'node:path';
import { runChatBridge } from '../chat.mjs';
import { MAX_INPUT_LINE } from '../chat-common.mjs';

if(process.env.AGENTDOCK_CHAT_TEST_INHERITED!=='kept')throw Error('This fixture is only available to the isolated chat test harness.');
let first=true,buffer=Buffer.alloc(0);
const rewrite=new Transform({
  transform(chunk,_encoding,callback){
    if(!first){callback(null,chunk);return;}
    buffer=Buffer.concat([buffer,chunk]);const newline=buffer.indexOf(10);
    if((newline<0?buffer.length:newline)>MAX_INPUT_LINE){callback(Error('Fixture init exceeded its limit.'));return;}
    if(newline<0){callback();return;}
    try{
      const job=JSON.parse(buffer.subarray(0,newline).toString('utf8'));
      if(job.type!=='init'||typeof job.cwd!=='string'||!isAbsolute(job.cwd))throw Error('Invalid fixture init.');
      job.program=join(job.cwd,'fake-chat-cli.mjs');
      this.push(JSON.stringify(job)+'\n');this.push(buffer.subarray(newline+1));buffer=Buffer.alloc(0);first=false;callback();
    }catch(error){callback(error);}
  },
});
const bridge=runChatBridge(rewrite);
process.stdin.pipe(rewrite);
const stop=()=>void bridge.shutdown();process.once('SIGTERM',stop);process.once('SIGINT',stop);
await bridge.done;
process.stdin.unpipe(rewrite);process.stdin.pause();rewrite.destroy();
process.removeListener('SIGTERM',stop);process.removeListener('SIGINT',stop);
