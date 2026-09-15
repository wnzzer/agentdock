// Deterministic transport fixture. Never launches a CLI or contacts a model.
import { createInterface } from 'node:readline';
let turn;
const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
for await (const line of createInterface({input:process.stdin})) {
  const job=JSON.parse(line);
  if(job.type==='init') emit({type:'ready',native_session_id:job.resume_id || 'fixture-native-thread'});
  if(job.type==='message') {
    turn=job.id;emit({type:'turn',id:turn,status:'running'});
    if(job.content==='APPROVAL') emit({type:'approval',id:'fixture-approval',title:'Fixture command approval',text:'A synthetic operation only',choices:['accept','decline','cancel']});
    else if(job.content!=='WAIT') {
      emit({type:'message',id:'answer-'+turn,role:'assistant',text:'fixture ',delta:true});
      emit({type:'message',id:'answer-'+turn,role:'assistant',text:'reply',delta:true});
      emit({type:'usage',input_tokens:3,output_tokens:2});
      emit({type:'turn',id:turn,status:'completed'});
    }
  }
  if(job.type==='approval') {emit({type:'approval_resolved',id:job.request_id});emit({type:'tool',id:'fixture-tool',name:'Fixture',status:job.decision==='accept'?'completed':'failed',text:job.decision});emit({type:'turn',id:turn,status:'completed'});}
  if(job.type==='interrupt') {emit({type:'approval_resolved',id:'fixture-approval'});emit({type:'turn',id:turn,status:'interrupted'});}
  if(job.type==='shutdown') {emit({type:'exit'});break;}
}
