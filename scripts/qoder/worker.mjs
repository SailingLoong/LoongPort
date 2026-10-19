import {pathToFileURL} from 'node:url';
import {complete,MAX_RESPONSE_BODY_BYTES} from './http-protocol.mjs';

const MAX_LINE=1024*1024;
const CHUNK_BYTES=48*1024;
const DRAIN_TIMEOUT_MS=2000;
const object=v=>v!==null&&typeof v==='object'&&!Array.isArray(v);
const only=(v,keys)=>object(v)&&Object.keys(v).every(k=>keys.includes(k));
const safeBinding=b=>only(b,['accountId','sessionId','turnId'])&&['accountId','sessionId','turnId'].every(k=>typeof b[k]==='string'&&b[k].length>0&&b[k].length<=1024&&!/[\x00-\x1f\x7f]/.test(b[k]));
const fixed=code=>Object.assign(new Error(code),{code});
const CODES=new Set(['QODER_INVALID_COMMAND','QODER_REQUEST_ABORTED','QODER_OUTCOME_UNKNOWN','QODER_MODEL_DIRECTORY_UNAVAILABLE','QODER_RESPONSE_TOO_LARGE','QODER_WORKER_FAILED']);

// One private pipe and one HTTP request lifecycle. No SDK or tool executor.
export async function runWorker({input=process.stdin,output=process.stdout}={}) {
  const controller=new AbortController();
  let started=false,finished=false,head=false,reader,task,pending='',binding;
  let transportFailed=false,cleanupFailed=false,stopCode='',total=0,outputUnavailable=false;
  let settle;const settled=new Promise(resolve=>{settle=resolve;});
  const abort=code=>{
    stopCode ||= code;transportFailed=true;
    if(!controller.signal.aborted)controller.abort(fixed(code));
    // Never await here: input handling must stay live during network/output waits.
    if(reader)void reader.cancel().catch(()=>{cleanupFailed=true;});
  };
  const write=frame=>new Promise((resolve,reject)=>{
    const bytes=JSON.stringify(frame)+'\n';
    if(Buffer.byteLength(bytes)>MAX_LINE){reject(fixed('QODER_WORKER_FAILED'));return;}
    let done=false;
    const finish=error=>{if(done)return;done=true;clearTimeout(timer);if(error)outputUnavailable=true;error?reject(fixed('QODER_WORKER_FAILED')):resolve();};
    const timer=setTimeout(()=>{abort('QODER_WORKER_FAILED');finish(true);output.destroy();},DRAIN_TIMEOUT_MS);
    try{output.write(bytes,error=>finish(error));}catch{finish(true);}
  });
  const emit=event=>write({kind:'event',event:{...binding,...event}});
  const finish=async()=>{
    if(finished)return;
    try{if(!outputUnavailable)await write({kind:'end',cleanupFailed,transportFailed});}catch{cleanupFailed=true;}
    finished=true;
    input.removeListener('data',onData);input.removeListener('end',onEnd);input.removeListener('error',onInputError);output.removeListener('error',onOutputError);
    // Keep a no-op output error listener while the process drains/destroys stdio.
    output.on('error',()=>{});
    input.destroy();settle();
  };
  const run=async request=>{
    try {
      if(controller.signal.aborted)throw fixed(stopCode||'QODER_REQUEST_ABORTED');
      const response=await complete(request.snapshot,request.chat,{signal:controller.signal});
      if(controller.signal.aborted){await response.body?.cancel();throw fixed(stopCode||'QODER_REQUEST_ABORTED');}
      // All metadata is constructed here. Never forward upstream headers/URLs.
      const streaming=response.headers.get('content-type')?.startsWith('text/event-stream');
      await emit({type:'response_head',status:response.status,headers:{'content-type':streaming?'text/event-stream':'application/json','cache-control':streaming?'no-cache':'no-store'}});
      head=true;
      reader=response.body?.getReader();
      while(reader && !controller.signal.aborted){
        const next=await reader.read();
        if(controller.signal.aborted || next.done)break;
        if(!(next.value instanceof Uint8Array))throw fixed('QODER_WORKER_FAILED');
        total+=next.value.byteLength;
        if(total>MAX_RESPONSE_BODY_BYTES){abort('QODER_RESPONSE_TOO_LARGE');throw fixed('QODER_RESPONSE_TOO_LARGE');}
        for(let at=0;at<next.value.byteLength&&!controller.signal.aborted;at+=CHUNK_BYTES){
          const data=Buffer.from(next.value.subarray(at,at+CHUNK_BYTES)).toString('base64');
          await emit({type:'response_chunk',data});
        }
      }
    } catch(error) {
      transportFailed=true;
      if(!controller.signal.aborted)controller.abort(fixed('QODER_WORKER_FAILED'));
      if(!head){const code=stopCode||error?.code;try{await write({kind:'error',code:CODES.has(code)?code:'QODER_WORKER_FAILED'});}catch{cleanupFailed=true;}}
    } finally {
      if(reader){try{await reader.cancel();}catch{cleanupFailed=true;}try{reader.releaseLock();}catch{cleanupFailed=true;}}
      reader=undefined;
      await finish();
    }
  };
  const invalid=()=>{
    abort('QODER_INVALID_COMMAND');
    if(!started){started=true;task=run({});}
  };
  const line=text=>{
    if(finished)return;
    let v;try{v=JSON.parse(text);}catch{invalid();return;}
    if(v?.kind==='cancel'){
      if(!started||!only(v,['kind'])){invalid();return;}
      abort('QODER_REQUEST_ABORTED');return;
    }
    if(v?.kind!=='start'||started||!only(v,['kind','request'])||!only(v.request,['binding','snapshot','chat'])||!safeBinding(v.request.binding)){invalid();return;}
    started=true;binding=Object.freeze({...v.request.binding});
    // Allows remaining commands already present in this input chunk to cancel
    // before starting network IO, without blocking future input events.
    task=Promise.resolve().then(()=>run(v.request));
  };
  const decoder=new TextDecoder('utf-8',{fatal:true});
  const onData=chunk=>{
    if(finished)return;
    try{pending+=decoder.decode(chunk,{stream:true});let n;while((n=pending.indexOf('\n'))>=0){if(Buffer.byteLength(pending.slice(0,n+1))>MAX_LINE){invalid();pending='';return;}const text=pending.slice(0,n);pending=pending.slice(n+1);line(text);}if(Buffer.byteLength(pending)>=MAX_LINE){invalid();pending='';}}catch{invalid();}
  };
  const onEnd=()=>{if(!finished){if(!started)invalid();else abort('QODER_REQUEST_ABORTED');}};
  const onInputError=()=>{if(!finished)invalid();};
  const onOutputError=()=>{if(!finished)abort('QODER_WORKER_FAILED');};
  input.on('data',onData);input.once('end',onEnd);input.on('error',onInputError);output.on('error',onOutputError);
  await settled;
  if(task)await task;
  return {outputUnavailable};
}

if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
  // No endpoint arguments, credentials in argv, environment config, or runtime SDK.
  if(process.argv.length===3 && process.argv[2]==='--check-runtime'){
    const ok=Number(process.versions.node.split('.')[0])>=22;
    process.stdout.write(JSON.stringify(ok?{ok:true,transport:'qoder-http',protocolVersion:1}:{ok:false,code:'QODER_NODE_VERSION_UNSUPPORTED'})+'\n');
    if(!ok)process.exitCode=1;
  } else if(Number(process.versions.node.split('.')[0])<22){
    process.stdout.write('{"kind":"error","code":"QODER_NODE_VERSION_UNSUPPORTED"}\n{"kind":"end","cleanupFailed":false,"transportFailed":true}\n');
    process.exitCode=1;
  } else if(process.argv.length!==2){process.stdout.write('{"kind":"error","code":"QODER_INVALID_COMMAND"}\n{"kind":"end","cleanupFailed":false,"transportFailed":true}\n');}
  else {const result=await runWorker();if(result.outputUnavailable)process.exit(0);}
}
