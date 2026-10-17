// Product task creation entry: official query({prompt,options}) -> owned bridge.
// Authentication is supplied by the caller; no env/CLI login fallback is selected here.
import {openExclusiveQueryTurn} from './query-bridge.mjs';
const loadOfficialSdk=()=>import('@qoder-ai/qoder-agent-sdk');
export async function startQoderTask({binding,prompt,options}, {loadSdk=loadOfficialSdk}={}) {
  if(typeof prompt!=='string'||!prompt.trim()||!binding||
     ['accountId','sessionId','turnId'].some(k=>typeof binding[k]!=='string'||!binding[k]))throw new Error('INVALID_TASK');
  if(!options||typeof options!=='object'||!options.auth)throw new Error('QODER_AUTH_REQUIRED');
  if(options.daemon!==undefined||options.resume!==undefined)throw new Error('QODER_EXCLUSIVE_TASK_REQUIRED');
  if(options.sessionId!==undefined&&options.sessionId!==binding.sessionId)throw new Error('QODER_SESSION_MISMATCH');
  // Freeze request selection before asynchronous SDK loading; auth is explicit
  // caller-owned data and is copied only in memory, never logged or persisted.
  const captured=Object.freeze({accountId:binding.accountId,sessionId:binding.sessionId,turnId:binding.turnId});
  let auth;
  try{auth=structuredClone(options.auth);}catch{throw new Error('QODER_AUTH_SNAPSHOT_REQUIRED');}
  const sdkOptions={...options,auth,sessionId:captured.sessionId};
  if(options.env){
    try{sdkOptions.env=structuredClone(options.env);}catch{throw new Error('INVALID_TASK');}
  }
  let query;
  try{
    const sdk=await loadSdk();
    query=sdk.query({prompt,options:sdkOptions});
  }catch{throw new Error('QODER_START_FAILED');}
  // Preserve bridge admission errors. In particular, never close a Query that
  // the SDK factory returned already bound to another external request.
  return openExclusiveQueryTurn({binding:captured,query});
}
