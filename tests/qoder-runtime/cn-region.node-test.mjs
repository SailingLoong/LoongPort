import assert from 'node:assert/strict';
import {test} from 'node:test';
import {admit,complete,_internal} from '../../scripts/qoder/http-protocol.mjs';
const snapshot=region=>({access:'synthetic-access',uid:'synthetic-user',machineId:'synthetic-machine',expiry:Date.now()+60000,region});
const chat={model:'fixture',messages:[{role:'user',content:'synthetic'}]};
test('CN is admitted without permitting caller-supplied hosts or unknown regions',()=>{
 assert.equal(admit(snapshot('cn'),chat),'');
 assert.equal(admit({...snapshot('cn'),origin:'https://api3.qoder.sh'},chat),'QODER_INVALID_SNAPSHOT');
 assert.equal(admit(snapshot('unknown'),chat),'QODER_INVALID_SNAPSHOT');
});
test('each regional request discovers models only at its fixed gateway',async()=>{
 const previous=globalThis.fetch; const seen=[];
 globalThis.fetch=async(url,options)=>{seen.push(String(url));assert.equal(options.redirect,'error');return Response.json({chat:[]});};
 try {
  for(const region of ['cn','international','cn']) {const r=await complete(snapshot(region),chat);assert.equal(r.status,400);}
  assert.deepEqual(seen.map(x=>new URL(x).origin),['https://gateway.qoder.com.cn','https://api3.qoder.sh','https://gateway.qoder.com.cn']);
 } finally {globalThis.fetch=previous;}
});
test('CN COSY header facts are distinct and body hashes match encoded bytes',async()=>{
 const previous=globalThis.fetch;const seen=[];
 const model={key:'fixture',display_name:'Fixture',enable:true,source:'system',max_input_tokens:100000};
 globalThis.fetch=async(url,options)=>{seen.push({url:String(url),options});if(!options.method)return Response.json({chat:[model]});return new Response('data: {"statusCodeValue":200,"body":"[DONE]"}\n\n',{headers:{'Content-Type':'text/event-stream'}});};
 try {
  await (await complete(snapshot('cn'),chat)).text();
  assert.equal(seen.length,2);
  for(const {options} of seen){assert.equal(options.headers['Cosy-ClientType'],'5');assert.equal(options.headers['Cosy-Version'],'1.0.0');assert.ok(options.headers['Cosy-Bodyhash']);}
  const {createHash}=await import('node:crypto');
  const decoded=JSON.parse(_internal.decodeBody(seen[1].options.body));
  assert.equal(decoded.session_type,'qodercli');assert.equal(decoded.business.product,'cli');
  assert.equal(seen[1].options.headers['Cosy-Bodyhash'],createHash('md5').update(seen[1].options.body).digest('hex'));
 } finally {globalThis.fetch=previous;}
});

test('CN multi-round streaming retains tool history and usage; a late failure cannot finish successfully', async()=>{
 const previous=globalThis.fetch;const posted=[];
 const model={key:'fixture',display_name:'Fixture',enable:true,source:'system',max_input_tokens:100000};
 const line=body=>`data: ${JSON.stringify({statusCodeValue:200,body:typeof body==='string'?body:JSON.stringify(body)})}\n\n`;
 const delta=(body,finish_reason=null,extra={})=>line({choices:[{index:0,delta:body,finish_reason}],...extra});
 const tool={id:'cn-call',type:'function',function:{name:'read_fixture',arguments:'{}'}};
 const replies=[delta({tool_calls:[{index:0,...tool}]})+delta({},'tool_calls')+line('[DONE]'),delta({content:'synthetic answer'})+delta({},'stop',{usage:{prompt_tokens:12,completion_tokens:4,total_tokens:16}})+line('[DONE]'),delta({content:'visible prefix'})+`data: ${JSON.stringify({statusCodeValue:500,body:'synthetic tail failure'})}\n\n`];
 globalThis.fetch=async(url,options)=>{
  assert.equal(new URL(url).origin,'https://gateway.qoder.com.cn');
  if(!options.method)return Response.json({chat:[model]});
  posted.push(JSON.parse(_internal.decodeBody(options.body)));
  assert.ok(replies.length,'no automatic inference retry');
  return new Response(replies.shift());
 };
 try {
  const tools=[{type:'function',function:{name:'read_fixture',parameters:{type:'object'}}}];
  const first=await (await complete(snapshot('cn'),{...chat,tools})).json();
  assert.deepEqual(first.choices[0].message.tool_calls,[tool]);
  const follow={...chat,tools,stream:true,messages:[...chat.messages,first.choices[0].message,{role:'tool',tool_call_id:'cn-call',content:'fixture result'},{role:'user',content:'summarize'}]};
  const second=await (await complete(snapshot('cn'),follow)).text();
  assert.match(second,/"total_tokens":16/);assert.match(second,/data: \[DONE\]/);
  assert.ok(posted[1].messages.some(m=>m.tool_call_id==='cn-call'&&m.content==='fixture result'));
  const failed=await (await complete(snapshot('cn'),follow)).text();
  assert.match(failed,/visible prefix/);assert.match(failed,/"error"/);
  assert.doesNotMatch(failed,/"finish_reason":"stop"|data: \[DONE\]/);
  assert.equal(posted.length,3);assert.equal(replies.length,0);
 } finally {globalThis.fetch=previous;}
});
