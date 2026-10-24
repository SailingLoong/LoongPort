// Synthetic transport fixture. Readiness, not elapsed startup time, arms Rust's
// virtual deadline tests. No networking, credentials, success end or DONE frame.
import './http-offline-guard.mjs';
import assert from 'node:assert/strict';
import {writeFileSync,renameSync} from 'node:fs';
import {createInterface} from 'node:readline';

const input=createInterface({input:process.stdin});
let prefix,started=false;
const publish=(suffix,value)=>{
  const target=prefix+suffix;
  writeFileSync(target+'.tmp',value);renameSync(target+'.tmp',target);
};
process.on('exit',code=>{if(prefix)publish('.exited',String(code));});
input.on('line',line=>{
  const command=JSON.parse(line);
  if(command.kind==='cancel'){
    assert.ok(started);
    publish('.cancelled','cancelled');
    input.close();process.stdin.destroy();return;
  }
  assert.equal(command.kind,'start');assert.equal(started,false);started=true;
  const request=command.request;
  prefix=request.testLifecyclePath;
  assert.equal(typeof prefix,'string');assert.ok(prefix.length>0);
  const mode=request.chat.model;
  assert.ok(['first','rejection','idle','nonstream'].includes(mode));
  const event=payload=>process.stdout.write(JSON.stringify({kind:'event',event:{...request.binding,...payload}})+'\n');
  if(mode!=='first'){
    event({type:'response_head',status:200,headers:{'content-type':mode==='nonstream'?'application/json':'text/event-stream'}});
    const body=mode==='rejection'?`data: ${JSON.stringify({error:{message:'QODER_DEVICE_REAUTHORIZATION_REQUIRED',code:401}})}\n\n`:mode==='idle'?'data: {"choices":[{"delta":{"content":"synthetic prefix"}}]}\n\n':'{"synthetic":"partial"}';
    event({type:'response_chunk',data:Buffer.from(body).toString('base64')});
  }
  publish('.ready',mode);
});
input.on('close',()=>process.stdin.destroy());
