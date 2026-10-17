process.stdout.write(JSON.stringify({kind:'event',event:{accountId:'account',sessionId:'session',turnId:'turn',type:'text_delta',text:'partial'}})+'\n');
process.exitCode=7;
