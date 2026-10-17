import test from 'node:test';
import assert from 'node:assert/strict';
const {openExclusiveQueryTurn}=await import('../../scripts/qoder/query-bridge.mjs').catch(()=>({openExclusiveQueryTurn:()=>{throw Error('QUERY_BRIDGE_NOT_IMPLEMENTED');}}));
const binding={accountId:'fixed-account',sessionId:'fixed-session',turnId:'fixed-turn'};
const tick=()=>new Promise(r=>setImmediate(r));
const result={type:'result',uuid:'synthetic-result',session_id:binding.sessionId,subtype:'success',is_error:false};
function source(){
 let wake;const queue=[];let closes=0;
 return {get closes(){return closes;},push:x=>{queue.push(x);wake?.();},async close(){closes++;queue.push({done:true});wake?.();},async *[Symbol.asyncIterator](){while(true){while(!queue.length)await new Promise(r=>wake=r);const x=queue.shift();if(x.error)throw x.error;if(x.done)return;yield x.value;}}};
}
test('Query bridge keeps result sealed until actual source EOF, then closes once',async()=>{
 const q=source(),turn=openExclusiveQueryTurn({binding,query:q});
 q.push({value:result});await tick();assert.equal(turn.snapshot().status,'success');assert.equal(q.closes,0);assert.equal(turn.diagnostics().lifecycle,'awaiting_end');
 q.push({done:true});await tick();assert.equal(q.closes,1);assert.equal(turn.diagnostics().lifecycle,'ended');turn.cancel();await tick();assert.equal(q.closes,1);
});
test('Query bridge cancellation closes owning Query exactly once without promoting late result',async()=>{
 const q=source(),turn=openExclusiveQueryTurn({binding,query:q});turn.cancel();turn.cancel();q.push({value:result});await tick();
 assert.equal(q.closes,1);assert.equal(turn.snapshot().status,'cancelled');assert.equal(turn.drainEvents().filter(e=>e.type==='terminal').length,1);
});
test('Query bridge fatal iterator error after result preserves success and records transport failure',async()=>{
 const q=source(),turn=openExclusiveQueryTurn({binding,query:q});q.push({value:result});await tick();q.push({error:Error('private source failure')});await tick();
 assert.equal(turn.snapshot().status,'success');assert.equal(turn.diagnostics().transportFailed,true);assert.equal(q.closes,1);assert.ok(!JSON.stringify(turn.diagnostics()).includes('private'));
});
test('result then cancel closes still-live exclusive source without rewriting success',async()=>{
 const q=source(),turn=openExclusiveQueryTurn({binding,query:q});q.push({value:result});await tick();
 turn.cancel();turn.cancel();await tick();assert.equal(q.closes,1);assert.equal(turn.snapshot().status,'success');assert.equal(turn.drainEvents().filter(e=>e.type==='terminal').length,1);
});
test('post-result close rejection is diagnosed without exposing private error or rewriting result',async()=>{
 const q=source();q.close=async()=>{throw Error('private credential-shaped cleanup failure');};
 const turn=openExclusiveQueryTurn({binding,query:q});q.push({value:result});await tick();turn.cancel();await tick();
 assert.equal(turn.snapshot().status,'success');assert.equal(turn.diagnostics().cleanupFailed,true);assert.equal(turn.diagnostics().lifecycle,'awaiting_end');assert.ok(!JSON.stringify(turn.diagnostics()).includes('private'));
});
test('same Query cannot be attached to a second account while first request owns it',async()=>{
 const q=source(),first=openExclusiveQueryTurn({binding,query:q});
 assert.throws(()=>openExclusiveQueryTurn({binding:{accountId:'other-account',sessionId:'other-session',turnId:'other-turn'},query:q}),/QUERY_ALREADY_BOUND/);
 assert.equal(q.closes,0);q.push({value:result});q.push({done:true});await tick();assert.equal(first.snapshot().status,'success');assert.equal(q.closes,1);
});
test('ended Query cannot be rebound merely because EOF arrived',async()=>{
 const q=source(),first=openExclusiveQueryTurn({binding,query:q});q.push({value:result});q.push({done:true});await tick();
 assert.throws(()=>openExclusiveQueryTurn({binding,query:q}),/QUERY_ALREADY_BOUND/);assert.equal(q.closes,1);assert.equal(first.snapshot().status,'success');
});
test('distinct Queries for same account remain independent and finish in reverse order',async()=>{
 const a=source(),b=source(),bindingB={...binding,sessionId:'session-B',turnId:'turn-B'};
 const first=openExclusiveQueryTurn({binding,query:a}),second=openExclusiveQueryTurn({binding:bindingB,query:b});
 b.push({value:{...result,session_id:bindingB.sessionId}});b.push({done:true});await tick();assert.equal(second.snapshot().status,'success');assert.equal(first.snapshot().status,'running');
 a.push({value:result});a.push({done:true});await tick();assert.equal(first.snapshot().status,'success');assert.deepEqual(second.snapshot().binding,bindingB);assert.equal(a.closes,1);assert.equal(b.closes,1);
});
test('source completion waits delayed cleanup rejection before publishing diagnostics',async()=>{
 let reject;const q=source();q.close=()=>new Promise((_,r)=>reject=r);
 const turn=openExclusiveQueryTurn({binding,query:q});let settled=false;
 void turn.waitForSourceEnd().then(()=>settled=true);q.push({done:true});await tick();
 assert.equal(settled,false);reject(Error('private'));await turn.waitForSourceEnd();
 assert.equal(turn.diagnostics().cleanupFailed,true);
});
