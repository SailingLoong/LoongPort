import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {startQoderTask} from '../../../scripts/qoder/task-entry.mjs';
await runWorker({startTask:req=>startQoderTask(req,{loadSdk:async()=>({query:({options})=>({
 async *[Symbol.asyncIterator](){yield {type:'assistant',uuid:'assistant-fixture',session_id:options.sessionId,message:{content:[{type:'text',text:'fixture-reply'}]}};yield {type:'result',uuid:'result-fixture',session_id:options.sessionId,subtype:'success',is_error:false};},async close(){}
})})})});
