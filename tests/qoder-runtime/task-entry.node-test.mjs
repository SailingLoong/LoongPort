import test from 'node:test';
import assert from 'node:assert/strict';
const {startQoderTask}=await import('../../scripts/qoder/task-entry.mjs').catch(()=>({startQoderTask:async()=>{throw Error('TASK_ENTRY_NOT_IMPLEMENTED');}}));
const binding={accountId:'account-A',sessionId:'session-A',turnId:'turn-A'};
const tick=()=>new Promise(r=>setImmediate(r));
const result={type:'result',uuid:'result-A',session_id:binding.sessionId,subtype:'success',is_error:false};
test('product task entry invokes SDK query and returns bridge events through source EOF',async()=>{
 let input,closes=0;
 const query={async *[Symbol.asyncIterator](){yield result;},async close(){closes++;}};
 const options={auth:{type:'accessToken',accessToken:'synthetic:admitted-pat'},cwd:'/synthetic/project',model:'synthetic-model'};
 const task=await startQoderTask({binding,prompt:'Explain the fixture',options},{loadSdk:async()=>({query:x=>{input=x;return query;}})});
 await tick();assert.equal(input.prompt,'Explain the fixture');assert.deepEqual(input.options,{...options,sessionId:binding.sessionId});assert.notEqual(input.options,options);
 assert.equal(task.snapshot().status,'success');assert.deepEqual(task.snapshot().binding,binding);assert.equal(task.drainEvents().filter(e=>e.type==='terminal').length,1);assert.equal(task.diagnostics().lifecycle,'ended');assert.equal(closes,1);
});
test('entry rejects missing explicit auth without loading SDK or ambient login',async()=>{
 let loads=0;await assert.rejects(startQoderTask({binding,prompt:'hi',options:{}},{loadSdk:async()=>{loads++;}}),/QODER_AUTH_REQUIRED/);assert.equal(loads,0);
});
test('entry validates binding and prompt before any SDK startup',async()=>{
 let loads=0;const deps={loadSdk:async()=>{loads++;}};
 await assert.rejects(startQoderTask({binding:{},prompt:'hi',options:{auth:{type:'accessToken',accessToken:'synthetic:pat'}}},deps),/INVALID_TASK/);
 await assert.rejects(startQoderTask({binding,prompt:' ',options:{auth:{type:'accessToken',accessToken:'synthetic:pat'}}},deps),/INVALID_TASK/);assert.equal(loads,0);
});
test('SDK startup errors are generic and do not expose credential-shaped messages',async()=>{
 await assert.rejects(startQoderTask({binding,prompt:'hi',options:{auth:{type:'accessToken',accessToken:'synthetic:pat'}}},{loadSdk:async()=>({query(){throw Error('private-token-value');}})}),e=>e.message==='QODER_START_FAILED'&&!JSON.stringify(e).includes('private'));
});
test('auth and account selection cannot drift while SDK module loads',async()=>{
 let load,seen;const auth={type:'accessToken',accessToken:'synthetic:pat',nested:{token:'synthetic:A'}};const mutable={...binding};
 const pending=startQoderTask({binding:mutable,prompt:'hi',options:{auth}},{loadSdk:()=>new Promise(r=>load=r)});
 auth.nested.token='synthetic:B';mutable.accountId='account-B';
 load({query:x=>{seen=x;return {async *[Symbol.asyncIterator](){yield result;},async close(){}};}});
 const task=await pending;await tick();assert.equal(seen.options.auth.nested.token,'synthetic:A');assert.equal(task.snapshot().binding.accountId,'account-A');assert.equal(task.snapshot().status,'success');
});
test('non-snapshotable auth is rejected before SDK loading',async()=>{
 let loads=0;await assert.rejects(startQoderTask({binding,prompt:'hi',options:{auth:{refresh(){}}}},{loadSdk:async()=>{loads++;}}),/QODER_AUTH_SNAPSHOT_REQUIRED/);assert.equal(loads,0);
});
test('conflicting SDK session selection is rejected before task startup',async()=>{
 let loads=0;await assert.rejects(startQoderTask({binding,prompt:'hi',options:{auth:{type:'accessToken',accessToken:'synthetic:pat'},sessionId:'foreign-session'}},{loadSdk:async()=>{loads++;}}),/QODER_SESSION_MISMATCH/);assert.equal(loads,0);
});
test('exclusive task entry cannot silently consume a daemon or resume session',async()=>{
 for(const extra of [{daemon:{}},{resume:'older-session'}]){
  let loads=0;await assert.rejects(startQoderTask({binding,prompt:'hi',options:{auth:{type:'accessToken',accessToken:'synthetic:pat'},...extra}},{loadSdk:async()=>{loads++;}}),/QODER_EXCLUSIVE_TASK_REQUIRED/);assert.equal(loads,0);
 }
});
