// Synthetic cancellation/EOF fixture for QoderHost, including its trusted temp marker.
import {createInterface} from 'node:readline';
import {writeFileSync} from 'node:fs';
process.stdout.on('error',()=>{});
const send=frames=>new Promise(resolve=>process.stdout.write(frames.map(frame=>JSON.stringify(frame)+'\n').join(''),()=>resolve()));
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
let binding,marker,finished=false;
const event=value=>({kind:'event',event:{accountId:binding.accountId,sessionId:binding.sessionId,turnId:binding.turnId,...value}});
async function finish(status,transportFailed=false){
  if(finished)return;finished=true;
  let cleanupFailed=false;
  try{if(marker)writeFileSync(marker,'closed');}catch{cleanupFailed=true;}
  await send([...(binding?[event({type:'terminal',status})]:[{kind:'error',code:'FIXTURE_INPUT_INVALID'}]),{kind:'end',cleanupFailed,transportFailed}]);
}
try {
  for await(const line of input){
    const command=JSON.parse(line);
    if(!binding){
      const value=command.request?.binding;
      if(command.kind!=='start'||!['accountId','sessionId','turnId'].every(key=>typeof value?.[key]==='string'&&value[key]))throw Error('FIXTURE_INPUT');
      binding={accountId:value.accountId,sessionId:value.sessionId,turnId:value.turnId};
      marker=command.request.options?.syntheticMarker;
      if(marker!==undefined&&typeof marker!=='string')throw Error('FIXTURE_INPUT');
      await send([event({type:'text_delta',text:'ready'})]);
    }else{
      if(command.kind!=='cancel'||Object.keys(command).length!==1)throw Error('FIXTURE_INPUT');
      await finish('cancelled');break;
    }
  }
  if(!finished)await finish(binding?'cancelled':'error',!binding);
}catch{await finish('error',true);}
input.close();process.stdin.destroy();
