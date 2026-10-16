import {createInterface} from 'node:readline';
const lines=createInterface({input:process.stdin});
lines.once('line',line=>{const {request}=JSON.parse(line);for(const event of [{type:'text_snapshot',messageId:'a',text:'partial'},{type:'terminal',status:'success'}])process.stdout.write(JSON.stringify({kind:'event',event:{...request.binding,...event}})+'\n');process.stdout.write('{"kind":"end","cleanupFailed":true,"transportFailed":false}\n');lines.close();process.stdin.destroy();});
