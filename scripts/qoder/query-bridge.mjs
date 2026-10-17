// Existing lifecycle wrapper's exclusive-Query connect implementation.
// Caller owns SDK startup/auth admission; this file neither imports nor launches SDK.
import {openTurn} from './lib/transport.mjs';
// No validated per-input request correlation or safe-rebind contract exists.
// Weak ownership records retain no Query, account, session, or credential payload.
const boundQueries=new WeakSet();
export function openExclusiveQueryTurn({binding,query}) {
  if(typeof query?.[Symbol.asyncIterator]!=='function'||typeof query?.close!=='function')throw new Error('INVALID_QUERY');
  if(boundQueries.has(query))throw new Error('QUERY_ALREADY_BOUND');
  let closeQuery,sourceEnded=false,closeFailed=false,resolveSource;
  const sourceEnd=new Promise(resolve=>resolveSource=resolve);
  const turn=openTurn({binding,mode:'sdk-exclusive',connect:callbacks=>{
    boundQueries.add(query);
    let closing;
    const closeOnce=()=>closing??=(Promise.resolve().then(()=>query.close()).catch(()=>{
      closeFailed=true;throw new Error('QUERY_CLOSE_FAILED');
    }));
    closeQuery=closeOnce;
    const abort=()=>{void closeOnce().catch(()=>{});};
    callbacks.signal.addEventListener('abort',abort,{once:true});
    if(callbacks.signal.aborted)abort();
    // A result is one message. Only iterator completion is source EOF.
    void (async()=>{
      try{
        for await(const message of query){
          if(!callbacks.signal.aborted)callbacks.onMessage(message);
        }
        sourceEnded=true;callbacks.onEnd();
      }catch{sourceEnded=true;callbacks.onError();}finally{
        // EOF admission includes the owned Query cleanup outcome.
        try{await closeOnce();}catch{}
        resolveSource();
      }
    })();
    return ()=>{
      callbacks.signal.removeEventListener('abort',abort);
      return closeOnce();
    };
  }});
  return Object.freeze({...turn,waitForSourceEnd:()=>sourceEnd,diagnostics:()=>{
    const facts=turn.diagnostics();return {...facts,cleanupFailed:facts.cleanupFailed||closeFailed};
  },cancel:()=>{
    turn.cancel();
    // UI result sealing must not make a still-live exclusive Query uncancellable.
    if(!sourceEnded)void closeQuery().catch(()=>{});
  }});
}
