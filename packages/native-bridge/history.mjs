import { withoutAgentDockSecrets } from "./native-spawn.mjs";
import { AppServerClient, AppServerError } from "./app-server.mjs";
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
  const environment=withoutAgentDockSecrets(process.env);
  environment.CODEX_HOME=configDir;
  const server=new AppServerClient(process.env.AGENTDOCK_CODEX_BIN||"codex",["app-server"],{cwd,env:environment});
  // A cancelled bridge must not leave its metadata-only app-server behind.
  const cancel=()=>{server.fail("Native history query cancelled");void server.close();};
  process.once("SIGTERM",cancel);process.once("SIGINT",cancel);
  const rpc=async(method,params)=>{
    try{return await server.request(method,params);}
    catch(error){throw error instanceof AppServerError?new Error("Native history API rejected the request; check client version."):error;}
  };
  let timer;
  try {
    return await Promise.race([
      (async()=>{
        await rpc("initialize",{clientInfo:{name:"agentdock_history",title:"AgentDock history",version:"0.1.0"}});
        server.notify("initialized");
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
    clearTimeout(timer);await server.close();
    process.removeListener("SIGTERM",cancel);process.removeListener("SIGINT",cancel);
  }
}
if(process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  try {let input="",bytes=0;const decoder=new StringDecoder("utf8");for await(const chunk of process.stdin){bytes+=chunk.length;if(bytes>32768)throw Error("Job too large");input+=decoder.write(chunk);}input+=decoder.end();
    const job=JSON.parse(input);const result=await discover(job);process.stdout.write(JSON.stringify(result));
  }catch(error){process.stdout.write(JSON.stringify({error:clean(error instanceof Error?error.message:"Native history query failed")}));process.exitCode=1;}
}
