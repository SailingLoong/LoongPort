// Synthetic QoderHost pipe fixture; intentionally independent of product runtime modules.
import {createInterface} from 'node:readline';
process.stdout.on('error',()=>{});
const send=frames=>new Promise(resolve=>process.stdout.write(frames.map(frame=>JSON.stringify(frame)+'\n').join(''),()=>resolve()));
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
let completed=false;
try {
  for await(const line of input){
    const command=JSON.parse(line),binding=command.request?.binding;
    if(command.kind!=='start'||!['accountId','sessionId','turnId'].every(key=>typeof binding?.[key]==='string'&&binding[key]))throw Error('FIXTURE_INPUT');
    const event=value=>({kind:'event',event:{accountId:binding.accountId,sessionId:binding.sessionId,turnId:binding.turnId,...value}});
    await send([event({type:'text_delta',text:'fixture-reply'}),event({type:'terminal',status:'success'}),{kind:'end',cleanupFailed:false,transportFailed:false}]);
    completed=true;break;
  }
} catch {}
if(!completed)await send([{kind:'error',code:'FIXTURE_INPUT_INVALID'},{kind:'end',cleanupFailed:false,transportFailed:true}]);
input.close();process.stdin.destroy();
