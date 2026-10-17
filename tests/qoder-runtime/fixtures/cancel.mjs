import {writeFileSync} from 'node:fs';
import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {startQoderTask} from '../../../scripts/qoder/task-entry.mjs';
await runWorker({startTask:req=>startQoderTask(req,{loadSdk:async()=>({query:({options})=>{
 let end;const stopped=new Promise(r=>end=r);
 return {async *[Symbol.asyncIterator](){yield {type:'assistant',uuid:'ready-fixture',session_id:options.sessionId,message:{content:[{type:'text',text:'ready'}]}};await stopped;},async close(){if(options.syntheticMarker)writeFileSync(options.syntheticMarker,'closed');end();}};
}})})});
