import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {startQoderTask} from '../../../scripts/qoder/task-entry.mjs';
await runWorker({startTask:req=>startQoderTask(req,{loadSdk:async()=>({query:()=>({async *[Symbol.asyncIterator](){throw Error('private synthetic failure');},async close(){}})})})});
