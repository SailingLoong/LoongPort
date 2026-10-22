// Production worker + synthetic in-memory upstream only. All real networking is denied.
import './http-offline-guard.mjs';
import assert from 'node:assert/strict';
import {Transform} from 'node:stream';
import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {_internal} from '../../../scripts/qoder/http-protocol.mjs';
let mode,pending='',posts=0;
const line=body=>`data: ${JSON.stringify({statusCodeValue:200,body:typeof body==='string'?body:JSON.stringify(body)})}\n\n`;
const delta=(content,finish_reason=null)=>line({choices:[{index:0,delta:{content},finish_reason}]});
globalThis.fetch=async(url,options)=>{
  const target=new URL(url);
  assert.equal(target.origin,'https://gateway.qoder.com.cn');assert.equal(options.redirect,'error');
  assert.equal(options.headers['Cosy-User'],'SYNTHETIC_CN_OWNER_UID');assert.equal(options.headers['Cosy-ClientType'],'5');
  if(!options.method){assert.equal(target.pathname,'/algo/api/v2/model/list');return Response.json({chat:[{key:mode,enable:true,source:'fixture',max_input_tokens:100000}]});}
  assert.equal(target.pathname,'/algo/api/v2/service/pro/sse/agent_chat_generation');assert.equal(++posts,1,'no inference replay');
  const body=JSON.parse(_internal.decodeBody(options.body));assert.equal(body.session_type,'qodercli');assert.equal(body.business.product,'cli');
  if(mode==='auth')return new Response('synthetic unauthorized',{status:401});
  if(mode==='quota')return Response.json({code:110,message:'synthetic limit'},{status:403});
  if(mode==='queue')return Response.json({code:403,message:JSON.stringify({code:10605,message:'synthetic busy'})},{status:403});
  if(mode==='request')return Response.json({code:406,message:'synthetic bad request'},{status:403});
  if(mode==='oversize')return new Response('x'.repeat(65537),{status:401});
  if(mode==='late-auth')return new Response(delta('visible prefix')+`data: ${JSON.stringify({statusCodeValue:403,body:JSON.stringify({code:403,message:'synthetic unauthorized'})})}\n\n`);
  if(mode==='late-quota')return new Response(delta('visible prefix')+`data: ${JSON.stringify({statusCodeValue:403,body:JSON.stringify({code:110,message:'synthetic limit'})})}\n\n`);
  return new Response(delta('synthetic CN success')+delta('','stop')+line('[DONE]'));
};
const gate=new Transform({transform(chunk,encoding,callback){try{
  pending+=chunk.toString('utf8');let n;
  while((n=pending.indexOf('\n'))>=0){
    const raw=pending.slice(0,n);pending=pending.slice(n+1);const command=JSON.parse(raw);
    if(command.kind==='start'){
      const snapshot=command.request.snapshot;
      assert.equal(Object.keys(snapshot).sort().join(','),'access,credentialKind,expiry,machineId,region,uid');
      assert.equal(snapshot.access,'SYNTHETIC_CN_OWNER_ACCESS');assert.equal(snapshot.uid,'SYNTHETIC_CN_OWNER_UID');
      assert.equal(snapshot.region,'cn');assert.equal(snapshot.credentialKind,'china_qoder_device');
      mode=command.request.chat.model;
    }
    this.push(raw+'\n');
  }
  callback();
}catch(error){callback(error);}}});
process.stdin.pipe(gate);
const result=await runWorker({input:gate,output:process.stdout});
process.stdin.destroy();if(result.outputUnavailable)process.exit(0);
