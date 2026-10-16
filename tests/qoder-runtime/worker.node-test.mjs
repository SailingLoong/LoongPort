import test from 'node:test';
import assert from 'node:assert/strict';
import {PassThrough} from 'node:stream';
const {runWorker}=await import('../../scripts/qoder/worker.mjs').catch(()=>({runWorker:async()=>{throw Error('WORKER_NOT_IMPLEMENTED');}}));
const tick=()=>new Promise(r=>setImmediate(r));
function channel(){const input=new PassThrough(),output=new PassThrough();let text='';output.on('data',b=>text+=b);return{input,output,frames:()=>text.trim().split('\n').filter(Boolean).map(JSON.parse)};}
test('worker forwards events and waits real source end, not presentation terminal',async()=>{
 const c=channel();let end;const events=[{type:'terminal',status:'success'}];
 const task={drainEvents:()=>events.splice(0),diagnostics:()=>({cleanupFailed:false}),waitForSourceEnd:()=>new Promise(r=>end=r),cancel(){}};
 const done=runWorker({...c,startTask:async()=>task});c.input.write(JSON.stringify({kind:'start',request:{}})+'\n');await tick();assert.equal(c.frames().some(x=>x.kind==='end'),false);end();await done;
 assert.deepEqual(c.frames().map(x=>x.kind),['event','end']);
});
test('cancel while startup pending is delivered once after task creation',async()=>{
 const c=channel();let ready,end,cancels=0;const created=new Promise(r=>ready=r);
 const done=runWorker({...c,startTask:()=>created});c.input.write('{"kind":"start","request":{}}\n{"kind":"cancel"}\n{"kind":"cancel"}\n');await tick();
 ready({drainEvents:()=>[],diagnostics:()=>({}),waitForSourceEnd:()=>new Promise(r=>end=r),cancel(){cancels++;}});await tick();assert.equal(cancels,1);end();await done;
});
test('startup error produces generic error frame without private detail',async()=>{
 const c=channel();const done=runWorker({...c,startTask:async()=>{throw Error('private-key');}});c.input.write('{"kind":"start","request":{}}\n');await done;
 assert.deepEqual(c.frames(),[{kind:'error',code:'QODER_TASK_FAILED'}]);
});
test('duplicate start rejection cannot launch a task after worker has already ended',async()=>{
 const c=channel();let starts=0;
 const done=runWorker({...c,startTask:async()=>{starts++;return{cancel(){},drainEvents:()=>[],waitForSourceEnd:async()=>{},diagnostics:()=>({})};}});
 c.input.write('{"kind":"start","request":{}}\n{"kind":"start","request":{}}\n');await done;await tick();
 assert.equal(starts,0);assert.deepEqual(c.frames(),[{kind:'error',code:'QODER_TASK_FAILED'}]);
});
