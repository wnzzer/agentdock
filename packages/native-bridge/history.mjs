import { spawn } from "node:child_process";
import { realpath, stat } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { StringDecoder } from "node:string_decoder";

const LIMIT=500;
const clean=(value,max=240)=>typeof value==="string"?value.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g," ").slice(0,max):"";
export async function normalizeSessions(rows,provider,cwd) {
  const result=[];
  for(const row of rows) {
    const id=provider==="codex"?row.id:row.sessionId;
    if(typeof id!=="string"||!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(id))continue;
    // Codex thread/list always carries cwd. Never label an unscoped/old-client row
    // as belonging to this workspace just because it omitted that field.
    if(provider==="codex"&&(typeof row.cwd!=="string"||!row.cwd))continue;
    let directory=cwd;
    if(typeof row.cwd==="string") {try {directory=await realpath(row.cwd);}catch {directory=resolve(row.cwd);}}
    if(directory!==cwd)continue;
    const rawTime=provider==="codex"?row.updatedAt*1000:row.lastModified;
    const date=new Date(rawTime);if(!Number.isFinite(date.getTime()))continue;
    result.push({id,provider,title:clean(row.customTitle||row.name||row.summary||row.preview||row.firstPrompt)||id,cwd:directory,updated_at:date.toISOString()});
  }
  return result;
}
export async function discover(job) {
  const cwd=await realpath(job.cwd),configDir=await realpath(job.config_dir);
  if(!(await stat(cwd)).isDirectory()||!(await stat(configDir)).isDirectory())throw Error("History source is not a directory");
  if(job.provider==="claude_code") {
    process.env.CLAUDE_CONFIG_DIR=configDir;
    // listSessions is a metadata-only SDK helper; no query() or tool loop is started.
    const { listSessions }=await import("@anthropic-ai/claude-agent-sdk");
    const rows=await listSessions({dir:cwd,includeWorktrees:false,limit:LIMIT+1});
    return {items:await normalizeSessions(rows.slice(0,LIMIT),"claude_code",cwd),truncated:rows.length>LIMIT};
  }
  if(job.provider!=="codex")throw Error("Unsupported history provider");
  const environment=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith("AGENTDOCK_SECRET_")&&key!=="AGENTDOCK_TOKEN"));
  environment.CODEX_HOME=configDir;
  const child=spawn(process.env.AGENTDOCK_CODEX_BIN||"codex",["app-server"],{cwd,env:environment,stdio:["pipe","pipe","pipe"]});
  child.stderr.on("data",()=>{}); // never echo native config/auth diagnostics to the API
  let sequence=0,buffer="",total=0;const decoder=new StringDecoder("utf8");
  const pending=new Map();
  const fail=(message)=>{for(const p of pending.values())p.reject(new Error(message));pending.clear();};
  child.stdin.on("error",()=>fail("Native history input closed"));
  // A cancelled bridge must not leave its metadata-only app-server behind.
  const cancel=()=>{fail("Native history query cancelled");child.kill("SIGTERM");};
  process.once("SIGTERM",cancel);process.once("SIGINT",cancel);
  child.once("error",()=>fail("Could not start Codex app-server"));
  child.once("exit",()=>fail("Codex history process exited"));
  child.stdout.on("data",chunk=>{
    total+=chunk.length;if(total>4*1024*1024){fail("History response exceeds limit");child.kill();return;}
    buffer+=decoder.write(chunk);
    let index;
    while((index=buffer.indexOf("\n"))!==-1) {
      const line=buffer.slice(0,index);buffer=buffer.slice(index+1);let message;
      try{message=JSON.parse(line);}catch{continue;}
      const p=pending.get(message.id);if(p){pending.delete(message.id);message.error?p.reject(new Error("Native history API rejected the request; check client version.")):p.resolve(message.result);}
    }
  });
  const rpc=(method,params)=>new Promise((resolve,reject)=>{const id=++sequence;pending.set(id,{resolve,reject});child.stdin.write(JSON.stringify({id,method,params})+"\n",err=>{if(err)fail("Native history input closed");});});
  let timer;
  try {
    return await Promise.race([
      (async()=>{
        await rpc("initialize",{clientInfo:{name:"agentdock_history",title:"AgentDock history",version:"0.1.0"}});
        child.stdin.write(JSON.stringify({method:"initialized",params:{}})+"\n");
        const items=[],seenIds=new Set(),seenCursors=new Set();let cursor=null,pages=0;
        do{
          const page=await rpc("thread/list",{cwd,limit:100,cursor,sortKey:"updated_at",modelProviders:[],sourceKinds:["cli","vscode","exec","appServer","unknown"],useStateDbOnly:true});
          for(const item of await normalizeSessions(Array.isArray(page.data)?page.data:[],"codex",cwd)) {
            if(!seenIds.has(item.id)){seenIds.add(item.id);items.push(item);}
          }
          cursor=page.nextCursor??null;
          if(cursor&&seenCursors.has(cursor))throw Error("Native history pagination did not advance");
          if(cursor)seenCursors.add(cursor);
          pages++;
        }while(cursor&&items.length<LIMIT&&pages<10);
        return {items:items.slice(0,LIMIT),truncated:!!cursor||items.length>LIMIT};
      })(),
      new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error("Native history query timed out")),12000);}),
    ]);
  }finally{
    clearTimeout(timer);child.stdin.end();child.kill("SIGTERM");
    await new Promise(resolve=>{if(child.exitCode!==null||child.signalCode){resolve();return;}const timer=setTimeout(()=>{child.kill("SIGKILL");resolve();},1500);child.once("exit",()=>{clearTimeout(timer);resolve();});});
    process.removeListener("SIGTERM",cancel);process.removeListener("SIGINT",cancel);
  }
}
if(process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  try {let input="",bytes=0;const decoder=new StringDecoder("utf8");for await(const chunk of process.stdin){bytes+=chunk.length;if(bytes>32768)throw Error("Job too large");input+=decoder.write(chunk);}input+=decoder.end();
    const job=JSON.parse(input);const result=await discover(job);process.stdout.write(JSON.stringify(result));
  }catch(error){process.stdout.write(JSON.stringify({error:clean(error instanceof Error?error.message:"Native history query failed")}));process.exitCode=1;}
}
