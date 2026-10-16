import {runWorker} from '../../../scripts/qoder/worker.mjs';
import {writeFileSync} from 'node:fs';
await runWorker({startTask:async req=>{let done;const pending=new Promise(r=>done=r);let events=[{...req.binding,type:'text_snapshot',messageId:'a',text:'ready'}];const history=JSON.parse(req.prompt.slice(req.prompt.indexOf('\n')+1));return{drainEvents:()=>events.splice(0),diagnostics:()=>({}),waitForSourceEnd:()=>pending,cancel(){writeFileSync(history[0].content,'closed');done();}};}});
