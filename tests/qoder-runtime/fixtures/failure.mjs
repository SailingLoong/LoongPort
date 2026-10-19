// Synthetic source failure at the QoderHost pipe boundary; no SDK/worker import.
import {createInterface} from 'node:readline';
process.stdout.on('error',()=>{});
const send=frames=>new Promise(resolve=>process.stdout.write(frames.map(frame=>JSON.stringify(frame)+'\n').join(''),()=>resolve()));
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
let completed=false;
try {
  for await(const line of input){
    const command=JSON.parse(line),binding=command.request?.binding;
    if(command.kind!=='start'||!['accountId','sessionId','turnId'].every(key=>typeof binding?.[key]==='string'&&binding[key]))throw Error('FIXTURE_INPUT');
    await send([{kind:'event',event:{accountId:binding.accountId,sessionId:binding.sessionId,turnId:binding.turnId,type:'terminal',status:'error'}},{kind:'end',cleanupFailed:false,transportFailed:true}]);
    completed=true;break;
  }
} catch {}
if(!completed)await send([{kind:'error',code:'FIXTURE_INPUT_INVALID'},{kind:'end',cleanupFailed:false,transportFailed:true}]);
input.close();process.stdin.destroy();
