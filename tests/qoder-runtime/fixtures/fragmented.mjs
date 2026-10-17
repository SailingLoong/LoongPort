process.stdin.resume();
process.stdout.write('{"kind":"event",');
setTimeout(()=>{process.stdout.write('"event":{"accountId":"account","sessionId":"session","turnId":"turn","text":"fragmented"}}\n');process.stdout.write('{"kind":"end","cleanupFailed":false,"transportFailed":false}\n');process.stdin.destroy();},150);
