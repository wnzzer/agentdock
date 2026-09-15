// Read-only official app-server probe. No user prompts or model inference.
import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

const config = await mkdtemp(join(tmpdir(), "agentdock-codex-probe-"));
const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !/^(OPENAI_|CODEX_|ANTHROPIC_|CLAUDE_|AGENTDOCK_SECRET_)/.test(key) && key !== "AGENTDOCK_TOKEN"));
environment.CODEX_HOME = config;
const child = spawn(process.env.AGENTDOCK_CODEX_BIN || "codex", ["app-server", "-c", 'cli_auth_credentials_store="file"'], { env: environment, stdio: ["pipe", "pipe", "pipe"] });
const lines=createInterface({ input:child.stdout });
let timer, next=1;
const pending=new Map();
child.stderr.on("data", () => {}); // diagnostics can include account/config details; don't print them
lines.on("line", line => {
  if (line.length > 2*1024*1024) return;
  let packet; try {packet=JSON.parse(line);} catch {return;}
  const request=pending.get(packet.id);
  if(request) {pending.delete(packet.id);packet.error?request.reject(new Error("Native RPC returned an error")):request.resolve(packet.result);}
});
function rpc(method,params) {return new Promise((resolve,reject)=>{const id=next++;pending.set(id,{resolve,reject});child.stdin.write(JSON.stringify({id,method,params})+"\n");});}
try {
  const result=await Promise.race([
    (async()=>{
      const init=await rpc("initialize",{clientInfo:{name:"agentdock_probe",title:"AgentDock protocol probe",version:"0.1.0"}});
      child.stdin.write(JSON.stringify({method:"initialized",params:{}})+"\n");
      const models=await rpc("model/list",{limit:20,includeHidden:false});
      return { initialized:!!init, model_count:Array.isArray(models?.data)?models.data.length:0, transport:"native app-server JSONL", inference_requested:false };
    })(),
    new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error("Native protocol probe timed out")),8000);}),
    new Promise((_,reject)=>child.once("error",()=>reject(new Error("Could not start Codex")))),
  ]);
  console.log(JSON.stringify(result));
} finally {
  clearTimeout(timer);lines.close();child.stdin.end();child.kill("SIGTERM");
  // Only the newly spawned probe is stopped. Existing user sessions are untouched.
}
