import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import {startQoderTask} from './task-entry.mjs';
import {loadExternalRuntime,SDK_VERSION} from './sdk-runtime.mjs';
const MAX_LINE=1024*1024;
// One private pipe, one task. No HTTP listener, credential file, or ambient login.
export function runWorker({input=process.stdin,output=process.stdout,startTask=startQoderTask}={}) {
  return new Promise(resolve=>{
    let task,started=false,cancelled=false,cancelSent=false,finished=false;
    const lines=createInterface({input,crlfDelay:Infinity});
    const finish=()=>{if(finished)return;finished=true;clearInterval(timer);lines.close();input.destroy();resolve();};
    const emit=frame=>{const line=JSON.stringify(frame)+'\n';if(Buffer.byteLength(line)>MAX_LINE)throw Error('FRAME_TOO_LARGE');output.write(line);};
    const fail=()=>{if(finished)return;try{emit({kind:'error',code:'QODER_TASK_FAILED'});}finally{task?.cancel();finish();}};
    const flush=()=>{if(task&&!finished)for(const event of task.drainEvents())emit({kind:'event',event});};
    const cancel=()=>{if(task&&cancelled&&!cancelSent){cancelSent=true;task.cancel();}};
    const timer=setInterval(()=>{try{flush();}catch{fail();}},20);
    lines.on('line',line=>{
      if(finished)return;
      try{
        if(Buffer.byteLength(line)>MAX_LINE)throw Error('FRAME_TOO_LARGE');
        const message=JSON.parse(line);
        if(message.kind==='cancel'&&started){cancelled=true;cancel();return;}
        if(message.kind!=='start'||started)throw Error('INVALID_COMMAND');
        started=true;
        void Promise.resolve().then(()=>finished?null:startTask(message.request)).then(async value=>{
          if(!value)return;
          if(finished){value.cancel();return;}
          task=value;cancel();await task.waitForSourceEnd();
          if(finished)return;flush();const facts=task.diagnostics();
          emit({kind:'end',cleanupFailed:facts.cleanupFailed===true,transportFailed:facts.transportFailed===true});finish();
        }).catch(fail);
      }catch{fail();}
    });
    lines.on('close',()=>{if(!finished)fail();});
    input.on('error',fail);output.on('error',fail);
  });
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
  const index=process.argv.indexOf('--sdk-root');
  const root=index<0?null:process.argv[index+1];
  const load=async()=>{
    if(Number(process.versions.node.split('.')[0])<22)throw Object.assign(Error('QODER_NODE_VERSION_UNSUPPORTED'),{code:'QODER_NODE_VERSION_UNSUPPORTED'});
    return loadExternalRuntime(root);
  };
  if(process.argv.includes('--check-runtime')){
    try{await load();process.stdout.write(JSON.stringify({ok:true,sdkVersion:SDK_VERSION,nodeVersion:process.versions.node})+'\n');}
    catch(error){const code=/^QODER_(SDK|NODE)_[A-Z_]+$/.test(error.code??'')?error.code:'QODER_SDK_LOAD_FAILED';process.stdout.write(JSON.stringify({ok:false,code})+'\n');process.exitCode=1;}
  }else{
    await runWorker({startTask:request=>startQoderTask(request,{loadSdk:async()=>{
      const runtime=await load();
      return {query:({prompt,options})=>runtime.sdk.query({prompt,options:{...options,transport:new runtime.sdk.WorkerTransport({pathToQoderWorkerRuntime:runtime.workerPath})}})};
    }})});
  }
}
