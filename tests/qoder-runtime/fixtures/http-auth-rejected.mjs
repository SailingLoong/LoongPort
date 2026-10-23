// Synthetic private transport-only responses. Never opens a socket or authenticates.
import {createInterface} from 'node:readline';
import {once} from 'node:events';
const input=createInterface({input:process.stdin});
const [line]=await once(input,'line');
const request=JSON.parse(line).request;
const mode=request.chat.model;
const fixed=mode==='expired'?'QODER_REAUTHORIZATION_REQUIRED':'QODER_DEVICE_REAUTHORIZATION_REQUIRED';
const event=payload=>process.stdout.write(JSON.stringify({kind:'event',event:{...request.binding,...payload}})+'\n');
event({type:'response_head',status:mode==='stream'?200:401,headers:{'content-type':mode==='stream'?'text/event-stream':'application/json'}});
const body=mode==='stream'?`data: ${JSON.stringify({error:{message:fixed,code:401}})}\n\n`:JSON.stringify({error:{message:fixed,type:'qoder_error',code:fixed,retryable:false}});
event({type:'response_chunk',data:Buffer.from(body).toString('base64')});
if(mode==='extra')event({type:'response_chunk',data:Buffer.from('unexpected').toString('base64')});
process.stdout.write(JSON.stringify({kind:'end',cleanupFailed:mode==='broken',transportFailed:false})+'\n');
input.close();process.stdin.destroy();
if(mode==='nonzero')process.exitCode=17;
