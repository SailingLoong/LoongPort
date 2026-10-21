// Test-only wrapper. The production worker has no origin/transport option.
import {Transform,Writable} from 'node:stream';
import {appendFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
import net from 'node:net';
import tls from 'node:tls';
import dgram from 'node:dgram';
import cp from 'node:child_process';
import {syncBuiltinESMExports} from 'node:module';

export async function runFixture({origin,tracePath,workerPath,terminalFault=null}) {
  if(![null,'nonzero','missing-end','extra-after-done','fragmented-jsonl','cleanup-failure'].includes(terminalFault))throw Error('FIXTURE_TERMINAL_FAULT');
  const endpoint=new URL(origin);
  if(endpoint.protocol!=='http:'||endpoint.hostname!=='127.0.0.1'||!endpoint.port||endpoint.pathname!=='/'||endpoint.search||endpoint.hash||endpoint.username||endpoint.password)throw Error('FIXTURE_ORIGIN');
  const trace=value=>appendFileSync(tracePath,JSON.stringify(value)+'\n');
  const realFetch=globalThis.fetch,connect=net.Socket.prototype.connect;
  const deny=()=>{throw Error('FIXTURE_NETWORK_DENIED');};
  net.Socket.prototype.connect=function(...args){const a=Array.isArray(args[0])?args[0][0]:args[0];if(!a||typeof a!=='object'||(a.host??a.hostname)!=='127.0.0.1'||Number(a.port)!==Number(endpoint.port))return deny();return connect.apply(this,args);};
  tls.connect=deny;dgram.createSocket=deny;net.Server.prototype.listen=deny;
  for(const k of ['spawn','exec','execFile','fork','spawnSync','execSync','execFileSync'])cp[k]=deny;
  syncBuiltinESMExports();
  let binding;
  const urls=new Map([
    ['https://api3.qoder.sh/algo/api/v2/model/list?Encode=1','GET'],
    ['https://api3.qoder.sh/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1','POST'],
  ]);
  const protocolURL=new URL('./http-protocol.mjs',pathToFileURL(workerPath));
  const {_internal}=await import(protocolURL.href);
  globalThis.fetch=(input,init={})=>{
    const url=new URL(String(input));const method=init.method??'GET';
    if(!binding||urls.get(url.href)!==method||init.redirect!=='error'||!init.signal)return deny();
    const capture={kind:'network',account:binding.accountId,method,cosyPresent:/^Bearer COSY\./.test(init.headers?.Authorization??''),signalPresent:true};
    if(method==='POST')capture.body=JSON.parse(_internal.decodeBody(init.body));
    trace(capture);
    return realFetch(new URL(url.pathname+url.search,endpoint),init);
  };
  let partial='';
  const gate=new Transform({transform(chunk,encoding,callback){try{partial+=chunk.toString('utf8');if(Buffer.byteLength(partial)>1048576)throw Error('FIXTURE_LINE');let n;while((n=partial.indexOf('\n'))>=0){const line=partial.slice(0,n);partial=partial.slice(n+1);const command=JSON.parse(line);if(command.kind==='start'){
    const r=command.request,s=r?.snapshot,b=r?.binding,id=b?.accountId;
    if(binding||typeof id!=='string'||!/^qoder-[a-z-]+$/.test(id)||s?.access!==`synthetic-http-access:${id}`||s.uid!==`synthetic-http-uid:${id}`||s.region!=='international'||!Number.isSafeInteger(s.expiry)||s.expiry<=Date.now()||!/^[a-f0-9-]{36}$/i.test(s.machineId??'')||Object.keys(s).sort().join(',')!=='access,expiry,machineId,region,uid')throw Error('FIXTURE_SYNTHETIC_SNAPSHOT_REQUIRED');
    binding={...b};trace({kind:'start',account:id,chat:r.chat});
  }this.push(line+'\n');}callback();}catch{callback(Error('FIXTURE_INPUT_DENIED'));}},flush(callback){if(partial)this.push(partial);callback();}});
  const {runWorker}=await import(pathToFileURL(workerPath).href);
  process.stdin.pipe(gate);
  let injectedAfterDone=false,fragmented=false;
  const output=terminalFault?new Writable({write(chunk,encoding,callback){try{
    const frame=JSON.parse(chunk.toString('utf8'));
    if(terminalFault==='fragmented-jsonl'&&!fragmented&&frame.kind==='event'&&frame.event?.type==='response_chunk'){
      fragmented=true;const cut=15;
      trace({kind:'fragmented-jsonl',account:binding?.accountId??null,splitAt:cut});
      process.stdout.write(chunk.subarray(0,cut),error=>{if(error){callback(error);return;}setTimeout(()=>process.stdout.write(chunk.subarray(cut),callback),50);});return;
    }
    if(terminalFault==='cleanup-failure'&&frame.kind==='end'){
      trace({kind:'cleanup-failure',account:binding?.accountId??null});
      process.stdout.write(JSON.stringify({...frame,cleanupFailed:true})+'\n',callback);return;
    }
    if(terminalFault==='missing-end'&&frame.kind==='end'){trace({kind:'withheld-end',account:binding?.accountId??null});callback();return;}
    if(terminalFault==='extra-after-done'){
      if(frame.kind==='event'&&frame.event?.type==='response_chunk'&&Buffer.from(frame.event.data,'base64').equals(Buffer.from('data: [DONE]\n\n'))){
        if(injectedAfterDone)throw Error('FIXTURE_DUPLICATE_DONE');
        injectedAfterDone=true;
        const extra={kind:'event',event:{...frame.event,data:Buffer.from('data: {"choices":[{"index":0,"delta":{"content":"EXTRA_AFTER_DONE_MUST_NOT_ESCAPE"},"finish_reason":null}]}\n\n').toString('base64')}};
        trace({kind:'injected-after-done',account:binding?.accountId??null});
        process.stdout.write(chunk,error=>{if(error){callback(error);return;}process.stdout.write(JSON.stringify(extra)+'\n',callback);});return;
      }
      if(frame.kind==='end'){
        if(!injectedAfterDone)throw Error('FIXTURE_DONE_REQUIRED');
        trace({kind:'bad-end',account:binding?.accountId??null});
        process.stdout.write(JSON.stringify({...frame,transportFailed:true})+'\n',callback);return;
      }
    }
    process.stdout.write(chunk,callback);
  }catch{callback(Error('FIXTURE_OUTPUT_FRAME'));}}}):process.stdout;
  const onStdoutError=error=>output.destroy(error);
  if(terminalFault)process.stdout.on('error',onStdoutError);
  const result=await runWorker({input:gate,output});
  if(terminalFault)process.stdout.removeListener('error',onStdoutError);
  trace({kind:'exit',account:binding?.accountId??null,outputUnavailable:result.outputUnavailable});
  process.stdin.destroy();
  if(result.outputUnavailable)process.exit(0);
  if(terminalFault==='nonzero')process.exitCode=17;
}
