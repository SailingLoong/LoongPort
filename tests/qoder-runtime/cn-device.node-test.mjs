import './fixtures/http-offline-guard.mjs';
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {admit,complete,_internal} from '../../scripts/qoder/http-protocol.mjs';

const snapshot=(extra={})=>({access:'synthetic-cn-device-access',uid:'synthetic-cn-device-user',machineId:'synthetic-cn-device-machine',expiry:Date.now()+60000,region:'cn',credentialKind:'china_qoder_device',...extra});
const chat={model:'fixture',messages:[{role:'user',content:'synthetic'}]};
const model={key:'fixture',display_name:'Fixture',enable:true,source:'system',max_input_tokens:100000};
const line=body=>`data: ${JSON.stringify({statusCodeValue:200,body:typeof body==='string'?body:JSON.stringify(body)})}\n\n`;
const answer=line({choices:[{index:0,delta:{content:'synthetic answer'},finish_reason:null}]})+line({choices:[{index:0,delta:{},finish_reason:'stop'}]})+line('[DONE]');

test('explicit CN device kind is accepted without token guessing; mismatched and missing kinds fail closed',()=>{
  assert.equal(admit(snapshot(),chat),'');
  for(const extra of [{region:'international'},{credentialKind:'international_qoder_pat'},{credentialKind:'official_client'},{credentialKind:undefined}])assert.equal(admit(snapshot(extra),chat),'QODER_INVALID_SNAPSHOT');
  assert.equal(admit(snapshot({credentialKind:'china_qoder_pat'}),chat),'');
  assert.equal(admit(snapshot({credentialKind:'international_qoder_pat',region:'international'}),chat),'');
});

test('CN device snapshot uses only fixed CN CLI model and chat contracts, never an auth or international endpoint',async()=>{
  const previous=globalThis.fetch,seen=[];
  globalThis.fetch=async(url,options)=>{
    seen.push({url:String(url),options});
    assert.equal(new URL(url).origin,'https://gateway.qoder.com.cn');
    assert.equal(options.redirect,'error');
    assert.equal(options.headers['Cosy-User'],'synthetic-cn-device-user');
    assert.equal(options.headers['Cosy-MachineId'],'synthetic-cn-device-machine');
    assert.equal(options.headers['Cosy-ClientType'],'5');
    assert.equal(options.headers['Cosy-Version'],'1.0.0');
    return !options.method?Response.json({chat:[model]}):new Response(answer);
  };
  try {
    const response=await complete(snapshot(),chat);
    assert.equal(response.status,200);
    assert.equal((await response.json()).choices[0].message.content,'synthetic answer');
    assert.deepEqual(seen.map(r=>new URL(r.url).pathname),['/algo/api/v2/model/list','/algo/api/v2/service/pro/sse/agent_chat_generation']);
    const body=JSON.parse(_internal.decodeBody(seen[1].options.body));
    assert.equal(body.session_type,'qodercli');assert.equal(body.business.product,'cli');
    assert.ok(!JSON.stringify(body).includes('synthetic-cn-device-access'));
  } finally {globalThis.fetch=previous;}
});

test('expired CN device requests explicitly require reauthorization without network IO',async()=>{
  const previous=globalThis.fetch;let calls=0;
  globalThis.fetch=async()=>{calls++;throw Error('must not fetch');};
  try {
    const response=await complete(snapshot({expiry:1}),chat);
    assert.equal(response.status,401);
    assert.equal((await response.json()).error.code,'QODER_REAUTHORIZATION_REQUIRED');
    assert.equal(calls,0);
  } finally {globalThis.fetch=previous;}
});

for(const stage of ['listing','chat'])for(const status of [401,403])test(`CN device ${stage} ${status} requires reauthorization, not refresh or replay`,async()=>{
  const previous=globalThis.fetch,seen=[];
  globalThis.fetch=async(url,options)=>{
    seen.push(String(url));
    assert.equal(new URL(url).origin,'https://gateway.qoder.com.cn');
    if(stage==='chat'&&!options.method)return Response.json({chat:[model]});
    return new Response('PRIVATE_REJECTION_BODY',{status});
  };
  try {
    const response=await complete(snapshot(),chat);
    assert.equal(response.status,401);
    const body=await response.json();
    assert.equal(body.error.code,'QODER_DEVICE_REAUTHORIZATION_REQUIRED');assert.equal(body.error.retryable,false);
    assert.ok(!JSON.stringify(body).includes('PRIVATE_REJECTION_BODY'));
    assert.equal(seen.length,stage==='listing'?1:2);
    assert.ok(seen.every(url=>!url.includes('jobToken')&&!url.includes('refresh')));
  } finally {globalThis.fetch=previous;}
});

for(const stream of [false,true])test(`CN device SSE auth rejection is explicitly signaled without replay (stream=${stream})`,async()=>{
  const previous=globalThis.fetch;let posts=0;
  globalThis.fetch=async(_,options)=>{
    if(!options.method)return Response.json({chat:[model]});posts++;
    const prefix=line({choices:[{index:0,delta:{content:'visible prefix'},finish_reason:null}]});
    return new Response(prefix+`data: ${JSON.stringify({statusCodeValue:401,body:'PRIVATE_DEVICE_REJECTION'})}\n\n`);
  };
  try {
    const response=await complete(snapshot(),{...chat,stream});
    const body=await response.text();
    assert.match(body,/QODER_DEVICE_REAUTHORIZATION_REQUIRED/);
    assert.doesNotMatch(body,/PRIVATE_DEVICE_REJECTION|data: \[DONE\]|"finish_reason":"stop"/);
    assert.equal(posts,1);
  } finally {globalThis.fetch=previous;}
});

for(const [code,expected] of [[10605,503],[110,429],[406,400]])for(const stage of ['listing','chat','sse'])test(`device business refusal ${code} on ${stage} is not an auth rejection`,async()=>{
  const previous=globalThis.fetch;let posts=0;
  const refused=JSON.stringify({code,message:'PRIVATE_BUSINESS_MESSAGE'});
  globalThis.fetch=async(_,options)=>{
    if(!options.method&&stage!=='listing')return Response.json({chat:[model]});
    if(options.method)posts++;
    return stage==='sse'?new Response(`data: ${JSON.stringify({statusCodeValue:403,body:refused})}\n\n`):new Response(refused,{status:403});
  };
  try {
    const response=await complete(snapshot(),chat);
    assert.equal(response.status,expected);
    const body=await response.text();
    assert.doesNotMatch(body,/REAUTHORIZATION_REQUIRED|PRIVATE_BUSINESS_MESSAGE/);
    assert.equal(posts,stage==='listing'?0:1);
  } finally {globalThis.fetch=previous;}
});

test('wrapped queue classification is not replaced by device auth marker',async()=>{
  const previous=globalThis.fetch;
  globalThis.fetch=async(_,options)=>!options.method?Response.json({chat:[model]}):Response.json({code:403,message:JSON.stringify({code:10605,message:'synthetic busy'})},{status:403});
  try {const response=await complete(snapshot(),chat);assert.equal(response.status,503);assert.doesNotMatch(await response.text(),/REAUTHORIZATION_REQUIRED/);}finally{globalThis.fetch=previous;}
});
for(const mode of ['oversized','unreadable'])test(`unverified ${mode} rejection body cannot invalidate device credentials`,async()=>{
  const previous=globalThis.fetch;let posts=0;
  globalThis.fetch=async(_,options)=>{
    if(!options.method)return Response.json({chat:[model]});posts++;
    return new Response(mode==='oversized'?'x'.repeat(65537):new ReadableStream({start(controller){controller.error(new Error('PRIVATE_READ_ERROR'));}}),{status:401});
  };
  try {await assert.rejects(()=>complete(snapshot(),chat),{message:'QODER_OUTCOME_UNKNOWN',code:'QODER_OUTCOME_UNKNOWN'});assert.equal(posts,1);}finally{globalThis.fetch=previous;}
});
