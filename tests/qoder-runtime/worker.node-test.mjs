import {capturePath} from './fixtures/http-capture-path.mjs';
import {gzipSync} from 'node:zlib';
import assert from 'node:assert/strict';
import {test,after} from 'node:test';
import http from 'node:http';
import {spawn} from 'node:child_process';
import {writeFileSync} from 'node:fs';
const ROOT=new URL('../../scripts/qoder/',import.meta.url);
const records=[];
const model={key:'fixture-model',source:'fixture',enable:true,max_input_tokens:100000};
const tools=[{type:'function',function:{name:'fixture_read',description:'Client fixture only',parameters:{type:'object',properties:{path:{type:'string'}},required:['path'],additionalProperties:false,$defs:{extra:{type:'number'}}}}}];
const binding={accountId:'account-fixture',sessionId:'session-fixture',turnId:'turn-fixture'};
const snapshot=()=>({access:'SYNTHETIC_HTTP_WORKER_ACCESS',uid:'fixture-user',machineId:'fixture-machine',expiry:Date.now()+3600000,region:'international',credentialKind:'international_qoder_pat'});
const chat=(more={})=>({model:model.key,messages:[{role:'user',content:'Read fixture'}],tools:structuredClone(tools),...more});
const start=(more={})=>({kind:'start',request:{binding:{...binding},snapshot:snapshot(),chat:chat(),...more}});
const env=body=>`data: ${JSON.stringify({statusCodeValue:200,body:typeof body==='string'?body:JSON.stringify(body)})}\n\n`;
const piece=(delta,finish_reason=null)=>env({choices:[{index:0,delta,finish_reason}]});
const done=env('[DONE]');
const plain=text=>piece({content:text})+piece({},'stop')+done;
const toolReply=piece({tool_calls:[{index:0,type:'function',id:'call_fixture',function:{name:'fixture_read',arguments:'{"path":"fixture.txt"}'}}]})+piece({},'tool_calls')+done;
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,ms=3000){const end=Date.now()+ms;while(!fn()&&Date.now()<end)await sleep(10);assert.ok(fn(),'bounded condition');}
async function bound(p,ms=5000){let timer;try{return await Promise.race([p,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('fixture deadline')),ms);})]);}finally{clearTimeout(timer);}}
function frames(text){return text.split('\n').filter(x=>x.startsWith('data: ')).map(x=>x.slice(6)).map(x=>x==='[DONE]'?x:JSON.parse(x));}
function bodyOf(fs){return Buffer.concat(fs.filter(f=>f.kind==='event'&&f.event.type==='response_chunk').map(f=>Buffer.from(f.event.data,'base64'))).toString('utf8');}
function ordered(fs){assert.equal(fs.filter(f=>f.kind==='end').length,1);assert.equal(fs.at(-1).kind,'end');const events=fs.filter(f=>f.kind==='event');if(events.length){assert.equal(events[0].event.type,'response_head');assert.equal(events.filter(f=>f.event.type==='response_head').length,1);for(const f of events){assert.equal(f.event.accountId,binding.accountId);assert.equal(f.event.sessionId,binding.sessionId);assert.equal(f.event.turnId,binding.turnId);if(f.event.type==='response_chunk'){assert.ok(Buffer.from(f.event.data,'base64').length<=49152);assert.equal(Buffer.from(f.event.data,'base64').toString('base64'),f.event.data);}}assert.deepEqual(Object.keys(events[0].event.headers).sort(),['cache-control','content-type']);}for(const f of fs)assert.ok(Buffer.byteLength(JSON.stringify(f))<1048576);}
async function fixture(label,handler){
 const record={label,requests:[],exit:null,stderr:'',frames:[],observations:{}};records.push(record);
 const sockets=new Set();
 const server=http.createServer(async(req,res)=>{const r={method:req.method,path:req.url,origin:req.headers['x-fixture-origin'],clientType:req.headers['cosy-clienttype'],cosyPresent:/^Bearer COSY\./.test(req.headers.authorization??''),machinePresent:!!req.headers['cosy-machineid'],userPresent:!!req.headers['cosy-user'],closed:false,premature:false};record.requests.push(r);res.on('close',()=>{r.closed=true;r.premature=!res.writableEnded;});if(req.method==='GET'){res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({chat:[model]}));return;}let wire='';for await(const b of req)wire+=b.toString();const {_internal}=await import('../../scripts/qoder/http-protocol.mjs');r.decodedBody=JSON.parse(_internal.decodeBody(wire));r.wireBytes=Buffer.byteLength(wire);res.writeHead(handler.status??200,{'content-type':'text/event-stream',...(handler.location?{location:handler.location}:{})});res.flushHeaders();await handler(req,res,r);});
 server.on('connection',s=>{sockets.add(s);s.on('close',()=>sockets.delete(s));});
 await new Promise(r=>server.listen({host:'127.0.0.1',port:0},r));
 const child=spawn(process.execPath,['--no-warnings',new URL('fixtures/http-worker-entry.mjs',import.meta.url).pathname,String(server.address().port)],{cwd:ROOT,env:{NODE_DISABLE_COMPILE_CACHE:'1'},stdio:['pipe','pipe','pipe']});
 let out='';child.stdout.on('data',b=>{out+=b.toString();let n;while((n=out.indexOf('\n'))>=0){record.frames.push(JSON.parse(out.slice(0,n)));out=out.slice(n+1);}});child.stderr.on('data',b=>{record.stderr+=b.toString();});child.stdin.on('error',()=>{});
 const exited=new Promise((resolve,reject)=>{child.once('error',reject);child.once('close',(code,signal)=>{record.exit={code,signal};resolve(record);});});
 const send=v=>child.stdin.write(JSON.stringify(v)+'\n');
 const finish=async()=>{await bound(exited);assert.deepEqual(record.exit,{code:0,signal:null});assert.equal(record.stderr,'');ordered(record.frames);assert.ok(!JSON.stringify(record.frames).includes('SYNTHETIC_HTTP_WORKER_ACCESS'));return record;};
 const clean=async()=>{if(record.exit===null){child.kill('SIGKILL');await bound(exited);}for(const s of sockets)s.destroy();server.closeAllConnections();await new Promise(r=>server.close(r));};
 return {record,child,send,finish,clean,post:()=>record.requests.filter(r=>r.method==='POST')};
}
after(()=>writeFileSync(capturePath('worker-captures.json.gz'),gzipSync(JSON.stringify({scope:'Real subprocess and IPv4 loopback only; synthetic snapshot; no real credential or account',records},null,2))));

test('real start produces one head, complete nonstream Chat bytes, end and exit zero',async()=>{const h=await fixture('basic',async(_,res)=>res.end(plain('fixture answer')));try{h.send(start());const r=await h.finish();assert.equal(r.frames[0].event.status,200);assert.equal(JSON.parse(bodyOf(r.frames)).choices[0].message.content,'fixture answer');assert.equal(h.post().length,1);assert.ok(r.requests.every(r=>r.cosyPresent&&r.machinePresent&&r.userPresent));assert.deepEqual(r.frames.at(-1),{kind:'end',cleanupFailed:false,transportFailed:false});}finally{await h.clean();}});

test('two processes return tools then transmit full call/result history and original schema',async()=>{let first;const h=await fixture('tool-round-one',async(_,res)=>res.end(toolReply));try{h.send(start());const r=await h.finish();first=JSON.parse(bodyOf(r.frames)).choices[0].message;assert.equal(first.tool_calls[0].id,'call_fixture');assert.equal(h.post().length,1);}finally{await h.clean();}let clientToolExecutions=0;const result=JSON.stringify({text:'fixture result',executions:++clientToolExecutions});const full=[{role:'system',content:'Keep full context'},{role:'user',content:'Read fixture'},first,{role:'tool',tool_call_id:'call_fixture',content:result},{role:'user',content:'Summarize'}];const second=await fixture('tool-round-two',async(_,res)=>res.end(plain('summary')));try{second.send(start({chat:chat({messages:full})}));const r=await second.finish();assert.equal(JSON.parse(bodyOf(r.frames)).choices[0].message.content,'summary');const b=second.post()[0].decodedBody;assert.deepEqual(b.tools,tools);assert.ok(b.messages.some(m=>m.role==='assistant'&&m.tool_calls?.[0].id==='call_fixture'));assert.ok(b.messages.some(m=>m.role==='tool'&&m.tool_call_id==='call_fixture'&&m.content===result));assert.ok(b.system[0].text.includes('Keep full context'));assert.equal(clientToolExecutions,1);}finally{await second.clean();}});

test('stream bytes stay standard Chat and finish once',async()=>{const h=await fixture('stream',async(_,res)=>res.end(plain('stream fixture')));try{h.send(start({chat:chat({stream:true})}));const r=await h.finish();const fs=frames(bodyOf(r.frames));assert.equal(fs.at(-1),'[DONE]');assert.equal(fs.filter(f=>f==='[DONE]').length,1);assert.equal(fs.filter(f=>f.choices?.[0]?.finish_reason==='stop').length,1);assert.ok(fs.some(f=>f.choices?.[0]?.delta?.content==='stream fixture'));}finally{await h.clean();}});

for(const mode of ['cancel','eof'])test(`${mode} while first tool frame pending aborts same request and reaps`,async()=>{const h=await fixture('pending-'+mode,async(_,res)=>res.write(piece({tool_calls:[{index:0,id:'pending',type:'function',function:{name:'fixture_read',arguments:'{'}}]})));try{h.send(start({chat:chat({stream:true})}));await until(()=>h.post().length===1);if(mode==='cancel')h.send({kind:'cancel'});else h.child.stdin.end();await until(()=>h.post()[0].closed);const r=await h.finish();assert.equal(h.post().length,1);assert.equal(h.post()[0].premature,true);assert.equal(r.frames.at(-1).transportFailed,true);}finally{await h.clean();}});

for(const mode of ['cancel','eof'])test(`${mode} remains live during stdout backpressure`,async()=>{const h=await fixture('backpressure-'+mode,async(_,res)=>{for(let i=0;i<100;i++)res.write(piece({content:'x'.repeat(16000)}));});try{h.child.stdout.pause();h.send(start({chat:chat({stream:true})}));await until(()=>h.post().length===1);await sleep(200);if(mode==='cancel')h.send({kind:'cancel'});else h.child.stdin.end();await until(()=>h.post()[0].closed);h.record.observations.serverClosedBeforeStdoutResume=true;h.child.stdout.resume();const r=await h.finish();assert.equal(h.post().length,1);assert.equal(h.post()[0].premature,true);assert.equal(r.frames.at(-1).transportFailed,true);}finally{await h.clean();}});

test('malformed choice and structure are rejected before any HTTP',async()=>{for(const change of [{tool_choice:'surprise'},{tool_choice:{type:'function',function:{name:'missing'}}},{parallel_tool_calls:'yes'},{messages:[{role:'alien',content:'x'}]},{tools:[{type:'custom',name:'x'}]}]){const h=await fixture('admission',async(_,res)=>res.end(plain('must not fetch')));try{h.send(start({chat:chat(change)}));const r=await h.finish();assert.equal(r.requests.length,0);assert.equal(r.frames[0].event.status,400);}finally{await h.clean();}}});

for(const bad of ['duplicate','foreign'])test(`${bad} command aborts instead of starting or switching binding`,async()=>{const h=await fixture(bad,async(_,res)=>res.write(piece({content:'prefix'})));try{h.send(start({chat:chat({stream:true})}));await until(()=>h.post().length===1);h.send(bad==='duplicate'?start({binding:{...binding,turnId:'foreign'}}):{kind:'cancel',binding:{...binding,turnId:'foreign'}});await until(()=>h.post()[0].closed);const r=await h.finish();assert.equal(h.post().length,1);assert.equal(r.frames.at(-1).transportFailed,true);assert.ok(!r.frames.some(f=>f.event?.turnId==='foreign'));}finally{await h.clean();}});

test('submitted transport failure has fixed nonretryable unknown outcome and one POST',async()=>{const h=await fixture('unknown-outcome',async(_,res)=>res.destroy());try{h.send(start());const r=await h.finish();assert.equal(h.post().length,1);assert.equal(r.frames[0].kind,'error');assert.equal(r.frames[0].code,'QODER_OUTCOME_UNKNOWN');assert.equal(r.frames.at(-1).transportFailed,true);}finally{await h.clean();}});

test('upstream error after output cannot produce successful finish and never leaks error text',async()=>{const h=await fixture('error-after-prefix',async(_,res)=>{res.write(piece({content:'safe prefix'}));res.end(`data: ${JSON.stringify({statusCodeValue:503,body:JSON.stringify({message:'PRIVATE_UPSTREAM_MARKER'})})}\n\n`);});try{h.send(start({chat:chat({stream:true})}));const r=await h.finish();const fs=frames(bodyOf(r.frames));assert.ok(fs.some(f=>f.error));assert.ok(!fs.includes('[DONE]'));assert.ok(!fs.some(f=>f.choices?.[0]?.finish_reason));assert.ok(!bodyOf(r.frames).includes('PRIVATE_UPSTREAM_MARKER'));assert.equal(h.post().length,1);}finally{await h.clean();}});

test('unknown-region and expired snapshots are rejected without HTTP',async()=>{for(const patch of [{region:'unverified-region'},{expiry:1}]){const h=await fixture('snapshot-admission',async(_,res)=>res.end(plain('no')));try{h.send(start({snapshot:{...snapshot(),...patch}}));const r=await h.finish();assert.equal(r.requests.length,0);assert.ok(r.frames[0].event.status>=400);}finally{await h.clean();}}});

test('permanently unread stdout has bounded shutdown after abort and no orphan process',async()=>{const h=await fixture('permanent-backpressure',async(_,res)=>{for(let i=0;i<100;i++)res.write(piece({content:'x'.repeat(16000)}));});try{h.child.stdout.pause();h.send(start({chat:chat({stream:true})}));await until(()=>h.post().length===1);await sleep(200);h.send({kind:'cancel'});await until(()=>h.post()[0].closed);await until(()=>h.child.exitCode!==null,4500);h.record.observations.processExitedWithoutStdoutConsumer=true;h.record.observations.endDeliveryImpossibleWhilePipeUnread=true;h.child.stdout.resume();await until(()=>h.record.exit!==null);assert.deepEqual(h.record.exit,{code:0,signal:null});assert.equal(h.record.stderr,'');assert.equal(h.post().length,1);}finally{await h.clean();}});

test('broken stdout aborts upstream and exits without private stderr',async()=>{const h=await fixture('broken-stdout',async(_,res)=>{res.write(piece({content:'prefix'}));const timer=setInterval(()=>res.write(piece({content:'continue'})),10);res.on('close',()=>clearInterval(timer));});try{h.send(start({chat:chat({stream:true})}));await until(()=>h.record.frames.some(f=>f.kind==='event'));h.child.stdout.destroy();await until(()=>h.post()[0].closed);await until(()=>h.record.exit!==null);assert.deepEqual(h.record.exit,{code:0,signal:null});assert.equal(h.record.stderr,'');assert.equal(h.post().length,1);}finally{await h.clean();}});

test('raw chunk splitting preserves UTF-8 bytes and the 48 KiB per-frame ceiling',async()=>{const text='合成🙂'.repeat(20000);const h=await fixture('chunk-limit',async(_,res)=>res.end(plain(text)));try{h.send(start());const r=await h.finish();assert.equal(JSON.parse(bodyOf(r.frames)).choices[0].message.content,text);assert.ok(r.frames.filter(f=>f.event?.type==='response_chunk').length>1);assert.ok(r.frames.filter(f=>f.event?.type==='response_chunk').every(f=>Buffer.from(f.event.data,'base64').length<=49152));}finally{await h.clean();}});

test('extra snapshot credentials and caller supplied origin are rejected before HTTP',async()=>{for(const patch of [{refresh:'FORBIDDEN_FIELD'},{pat:'FORBIDDEN_FIELD'},{origin:'https://caller.invalid'}]){const h=await fixture('snapshot-extra',async(_,res)=>res.end(plain('no')));try{h.send(start({snapshot:{...snapshot(),...patch}}));const r=await h.finish();assert.equal(r.requests.length,0);assert.equal(r.frames[0].event.status,400);assert.ok(!JSON.stringify(r.frames).includes('FORBIDDEN_FIELD'));}finally{await h.clean();}}});

test('explicit strict false is compatible while strict true and nonboolean are rejected',async()=>{for(const strict of [false,true,'false']){const h=await fixture('strict-'+strict,async(_,res)=>res.end(plain('strict false accepted')));try{const declared=structuredClone(tools);declared[0].function.strict=strict;h.send(start({chat:chat({tools:declared})}));const r=await h.finish();assert.equal(r.frames[0].event.status,strict===false?200:400);assert.equal(h.post().length,strict===false?1:0);if(strict===false)assert.deepEqual(h.post()[0].decodedBody.tools[0].function.parameters,tools[0].function.parameters);}finally{await h.clean();}}});

test('production --check-runtime subprocess is offline and independent of SDK/account/stdin',async()=>{const child=spawn(process.execPath,['--import',new URL('fixtures/http-offline-guard.mjs',import.meta.url).pathname,new URL('../../scripts/qoder/worker.mjs',import.meta.url).pathname,'--check-runtime'],{env:{NODE_DISABLE_COMPILE_CACHE:'1'},stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',b=>stdout+=b.toString());child.stderr.on('data',b=>stderr+=b.toString());const code=await bound(new Promise(resolve=>child.once('close',resolve)));assert.equal(code,0);assert.equal(stderr,'');assert.deepEqual(JSON.parse(stdout),{ok:true,transport:'qoder-http',protocolVersion:1});});

test('image history URL is only serialized to fixed upstream, never fetched locally',async()=>{const image='https://images.invalid/synthetic.png';const h=await fixture('image-passthrough',async(_,res)=>res.end(plain('image fixture')));try{h.send(start({chat:chat({messages:[{role:'user',content:[{type:'text',text:'look'},{type:'image_url',image_url:{url:image}}]}]})}));const r=await h.finish();assert.equal(r.frames[0].event.status,200);assert.equal(r.requests.length,2);assert.ok(h.post()[0].decodedBody.messages.some(m=>Array.isArray(m.content)&&m.content.some(x=>x.image_url?.url===image)));}finally{await h.clean();}});

test('redirect is not followed or replayed after a POST',async()=>{const handler=async(_,res)=>res.end('PRIVATE_REDIRECT_BODY');handler.status=307;handler.location='https://must-not-visit.invalid/path';const h=await fixture('redirect-denied',handler);try{h.send(start());const r=await h.finish();assert.equal(h.post().length,1);assert.equal(r.requests.length,2);assert.equal(r.frames[0].code,'QODER_OUTCOME_UNKNOWN');assert.ok(!JSON.stringify(r.frames).includes('must-not-visit'));assert.ok(!bodyOf(r.frames).includes('PRIVATE_REDIRECT_BODY'));}finally{await h.clean();}});

test('non-200 body is discarded and only fixed allowlisted metadata is forwarded',async()=>{const handler=async(_,res)=>res.end('PRIVATE_FAILURE_BODY');handler.status=429;const h=await fixture('http-error-fixed',handler);try{h.send(start());const r=await h.finish();assert.equal(r.frames[0].event.status,429);assert.equal(JSON.parse(bodyOf(r.frames)).error.message,'QODER_UPSTREAM_REJECTED');assert.ok(!bodyOf(r.frames).includes('PRIVATE_FAILURE_BODY'));assert.equal(h.post().length,1);}finally{await h.clean();}});

test('production input line limit rejects an oversized JSONL command without HTTP',async()=>{const child=spawn(process.execPath,['--import',new URL('fixtures/http-offline-guard.mjs',import.meta.url).pathname,new URL('../../scripts/qoder/worker.mjs',import.meta.url).pathname],{env:{NODE_DISABLE_COMPILE_CACHE:'1'},stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',b=>stdout+=b.toString());child.stderr.on('data',b=>stderr+=b.toString());child.stdin.on('error',()=>{});child.stdin.write(' '.repeat(1048576));const code=await bound(new Promise(resolve=>child.once('close',resolve)));assert.equal(code,0);assert.equal(stderr,'');const fs=stdout.trim().split('\n').map(JSON.parse);assert.deepEqual(fs,[{kind:'error',code:'QODER_INVALID_COMMAND'},{kind:'end',cleanupFailed:false,transportFailed:true}]);});


const cnDevice=()=>({...snapshot(),region:'cn',credentialKind:'china_qoder_device'});
test('CN device private pipe works in two real processes with full tools history and fixed regional URLs',async()=>{
  let assistant;
  const first=await fixture('cn-device-tools',async(_,res)=>res.end(toolReply));
  try {
    first.send(start({snapshot:cnDevice()}));
    const result=await first.finish();
    assert.equal(result.frames[0].event.status,200);
    assistant=JSON.parse(bodyOf(result.frames)).choices[0].message;
    assert.equal(assistant.tool_calls[0].id,'call_fixture');
    assert.equal(first.post().length,1);
    assert.ok(result.requests.every(r=>r.origin==='https://gateway.qoder.com.cn'&&r.clientType==='5'));
  } finally {await first.clean();}
  const second=await fixture('cn-device-followup',async(_,res)=>res.end(plain('CN followup')));
  try {
    const history=[...chat().messages,assistant,{role:'tool',tool_call_id:'call_fixture',content:'synthetic tool result'},{role:'user',content:'summarize'}];
    second.send(start({snapshot:cnDevice(),chat:chat({stream:true,messages:history})}));
    const result=await second.finish();
    const body=second.post()[0].decodedBody;
    assert.equal(body.session_type,'qodercli');assert.equal(body.business.product,'cli');
    assert.ok(body.messages.some(m=>m.role==='tool'&&m.tool_call_id==='call_fixture'&&m.content==='synthetic tool result'));
    assert.deepEqual(body.tools,tools);
    assert.equal(frames(bodyOf(result.frames)).at(-1),'[DONE]');
    assert.equal(second.post().length,1);
    assert.ok(result.requests.every(r=>r.origin==='https://gateway.qoder.com.cn'&&r.clientType==='5'));
  } finally {await second.clean();}
});

test('CN device late stream failure never finishes or retries inference',async()=>{
  const h=await fixture('cn-device-tail-failure',async(_,res)=>{
    res.write(piece({content:'CN visible prefix'}));
    res.end(`data: ${JSON.stringify({statusCodeValue:401,body:'PRIVATE_CN_EXPIRED_TOKEN'})}\n\n`);
  });
  try {
    h.send(start({snapshot:cnDevice(),chat:chat({stream:true})}));
    const result=await h.finish(),body=bodyOf(result.frames),stream=frames(body);
    assert.match(body,/CN visible prefix/);assert.ok(stream.some(f=>f.error));
    assert.ok(!stream.includes('[DONE]'));assert.ok(!stream.some(f=>f.choices?.[0]?.finish_reason));
    assert.doesNotMatch(body,/PRIVATE_CN_EXPIRED_TOKEN/);assert.equal(h.post().length,1);
    assert.ok(result.requests.every(r=>r.origin==='https://gateway.qoder.com.cn'));
  } finally {await h.clean();}
});

test('CN device cross-region private frame is rejected before any worker network request',async()=>{
  const h=await fixture('cn-device-region-mismatch',async(_,res)=>res.end(plain('must not submit')));
  try {
    h.send(start({snapshot:{...cnDevice(),region:'international'}}));
    const result=await h.finish();assert.equal(result.frames[0].event.status,400);assert.equal(result.requests.length,0);
  } finally {await h.clean();}
});
