import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp,rm,realpath,readFile,chmod,copyFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { normalizeSessions,discover } from './history.mjs';

test('native metadata is scoped, bounded, and contains no transcript payload',async()=>{
  const root=await realpath(await mkdtemp(join(tmpdir(),'ad-history-test-')));
  try {
    const rows=await normalizeSessions([{id:'abc-123',name:'test',preview:'private transcript',cwd:root,updatedAt:1700000000},{id:'other',cwd:'/another',updatedAt:1700000000},{id:'--bad',cwd:root,updatedAt:1700000000},{id:'unscoped',updatedAt:1700000000},{id:'empty',cwd:'',updatedAt:1700000000}], 'codex',root);
    assert.equal(rows.length,1);assert.equal(rows[0].title,'test');assert.equal(rows[0].preview,undefined);
  }finally{await rm(root,{recursive:true});}
});

async function runCodexFixture(repeatedCursor=false) {
  const root=await realpath(await mkdtemp(join(tmpdir(),'ad-codex-transport-')));
  const fixture=join(root,'codex-history-cli.mjs');
  await copyFile(fileURLToPath(new URL('./fixtures/codex-history-cli.mjs',import.meta.url)),fixture);
  await chmod(fixture,0o755);
  const log=join(root,'requests.log');
  const child=spawn(process.execPath,[fileURLToPath(new URL('./history.mjs',import.meta.url))],{
    env:{...process.env,AGENTDOCK_CODEX_BIN:fixture,AGENTDOCK_SECRET_TEST_TOKEN:'fake-test-secret',AGENTDOCK_TOKEN:'fake-test-token',AGENTDOCK_HISTORY_TEST_LOG:log,AGENTDOCK_HISTORY_TEST_REPEAT:repeatedCursor?'1':'0'},
    stdio:['pipe','pipe','pipe'],
  });
  const chunks=[];child.stdout.on('data',chunk=>chunks.push(chunk));child.stderr.resume();
  const timer=setTimeout(()=>child.kill('SIGKILL'),5000);
  try {
    const exited=new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',resolve);});
    child.stdin.end(JSON.stringify({provider:'codex',cwd:root,config_dir:root}));
    const code=await exited;
    return {code,result:JSON.parse(Buffer.concat(chunks).toString()),methods:(await readFile(log,'utf8')).trim().split('\n')};
  }finally{clearTimeout(timer);child.kill('SIGKILL');await rm(root,{recursive:true});}
}

test('Node bridge uses only native metadata RPCs, scopes pages and preserves streamed Unicode',async()=>{
  const {code,result,methods}=await runCodexFixture();
  assert.equal(code,0);assert.equal(result.items.length,1);assert.equal(result.items[0].title,'修复 🦊 布局');
  assert.equal(result.truncated,false);
  assert.deepEqual(methods,['initialize','initialized','thread/list','thread/list']);
});

test('Node bridge rejects stuck pagination without an unbounded query loop',async()=>{
  const {code,result,methods}=await runCodexFixture(true);
  assert.equal(code,1);assert.match(result.error,/pagination did not advance/);
  assert.equal(methods.filter(method=>method==='thread/list').length,2);
});
test('official Claude metadata helper handles an empty configured source without a model call',async()=>{
  const root=await mkdtemp(join(tmpdir(),'ad-empty-claude-'));
  const previous=process.env.CLAUDE_CONFIG_DIR;
  try {const result=await discover({provider:'claude_code',cwd:root,config_dir:root});assert.deepEqual(result.items,[]);}
  finally{if(previous===undefined)delete process.env.CLAUDE_CONFIG_DIR;else process.env.CLAUDE_CONFIG_DIR=previous;await rm(root,{recursive:true});}
});
