/** Host adapter candidate; SDK-shaped modes do not import/run SDK code. */
import {createState,convert} from './converter.mjs';
export function openTurn({binding,connect,mode='synthetic',interrupt}) {
  if(!['synthetic','sdk-exclusive','sdk-shared'].includes(mode))throw new Error('INVALID_MODE');
  if(mode==='sdk-shared'&&typeof interrupt!=='function')throw new Error('MISSING_INTERRUPT');
  let state=createState(binding);
  const captured=Object.freeze({...state.binding}),controller=new AbortController();
  let sealed=false,releaseReady=false,dispose=null,disposed=false,hostId=0,events=[],failure=null,cleanupFailed=false;
  let lifecycle='open',interruptState='not_requested',transportFailed=false;
  function release(){
    if(!releaseReady||!dispose||disposed)return;
    disposed=true;
    try {Promise.resolve(dispose()).catch(()=>{cleanupFailed=true;});}catch{cleanupFailed=true;}
  }
  function requestInterrupt(){
    if(interruptState!=='not_requested')return;
    interruptState='pending';
    try {Promise.resolve(interrupt()).then(()=>{interruptState='acknowledged';},()=>{interruptState='failed';});}
    catch{interruptState='failed';}
  }
  function seal(cause){
    sealed=true; // Must precede abort, interrupt, and disposal: each can reenter.
    if(mode==='synthetic'||(mode==='sdk-exclusive'&&cause==='cancel')){
      releaseReady=true;lifecycle='ended';controller.abort();release();
    }else if(cause==='end'){
      releaseReady=true;lifecycle='ended';release();
    }else{
      lifecycle='awaiting_end';
      if(mode==='sdk-shared'&&cause==='cancel')requestInterrupt();
    }
  }
  function apply(envelope,cause='result'){
    const next=convert(state,envelope);state=next.state;events.push(...next.events);
    if(state.finished)seal(cause);
  }
  function stop(kind,cause='cancel'){
    if(sealed)return;
    try {apply({...captured,kind,id:'transport-'+(++hostId)},cause);}
    catch{
      state={...state,status:state.status==='running'?(kind==='cancel'?'cancelled':'error'):state.status,finished:true};
      events.push({...captured,type:'terminal',status:state.status,reason:kind});seal(cause);
    }
  }
  function finishTransport(isError){
    if(isError)transportFailed=true;
    if(!sealed)stop(isError?'error':'end','end');
    else if(mode!=='synthetic'){releaseReady=true;lifecycle='ended';release();}
  }
  function onMessage(message){
    if(sealed)return;
    try {
      const copied=structuredClone(message);
      apply({...captured,kind:'sdk',message:copied});
      // Validate via reducer first, preserve observed text, then prevent success promotion.
      if(copied.type==='assistant'&&copied.aborted===true&&!sealed){failure='ASSISTANT_ABORTED';stop('error','result');}
    }catch(error){
      failure=['SESSION_MISMATCH','MISSING_STAGE','EVENT_ID_CONFLICT','TOOL_PARENT_MISMATCH','TOOL_ID_CONFLICT'].includes(error?.message)?error.message:'CONVERSION_REJECTED';
      stop('error');
    }
  }
  const callbacks=Object.freeze({binding:captured,signal:controller.signal,onMessage,onError:()=>finishTransport(true),onEnd:()=>finishTransport(false)});
  function register(value){
    if(typeof value!=='function'){failure??='INVALID_DISPOSER';stop('error');return;}
    dispose=value;release();
  }
  try{
    const resource=connect(callbacks);
    if(typeof resource==='function')register(resource);
    else Promise.resolve(resource).then(register,()=>finishTransport(true));
  }catch{finishTransport(true);}
  return Object.freeze({cancel:()=>stop('cancel'),snapshot:()=>structuredClone(state),drainEvents:()=>{const batch=structuredClone(events);events=[];return batch;},diagnostics:()=>({failure,cleanupFailed,lifecycle,interrupt:interruptState,safeToRebind:false,transportFailed,mode})});
}
