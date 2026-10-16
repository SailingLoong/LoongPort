import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {startQoderTask} from '../../../scripts/qoder/task-entry.mjs';
await runWorker({startTask:req=>startQoderTask(req,{loadSdk:async()=>({query:({prompt,options})=>({
 async *[Symbol.asyncIterator](){const history=JSON.parse(prompt.slice(prompt.indexOf('\n')+1));if(history.length!==3||history[1].content!=='prior'||history[2].content!=='second'||options.auth.accessToken!=='synthetic:pat'||options.tools.length!==0)throw Error('bad-request');yield {type:'assistant',uuid:'a',session_id:options.sessionId,message:{content:[{type:'text',text:'history-and-auth-ok'}]}};yield {type:'result',uuid:'r',session_id:options.sessionId,subtype:'success',is_error:false};},async close(){}
})})})});
