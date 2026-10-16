/** Offline, side-effect-free conversion of a deliberately bounded SDK subset.
 * One reducer state per trusted (account, session, turn). See CONTRACT.md.
 * No SDK, filesystem, network, process, auth or tool-execution imports.
 */
const fail = code => { throw new Error(code); };
const string = (x, code) => typeof x === 'string' && x.length > 0 ? x : fail(code);
const canonical = value => {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number' && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && Object.getPrototypeOf(value) === Object.prototype)
    return '{' + Object.keys(value).sort().map(k => JSON.stringify(k) + ':' + canonical(value[k])).join(',') + '}';
  return fail('NON_JSON_INPUT');
};
export function createState(binding) {
  for (const key of ['accountId', 'sessionId', 'turnId']) string(binding?.[key], 'MISSING_BINDING');
  return {binding: {accountId:binding.accountId, sessionId:binding.sessionId, turnId:binding.turnId}, seen:[], messages:[], tools:[], stages:[], status:'running', finished:false};
}
export function convert(previous, envelope) {
  for (const key of ['accountId', 'sessionId', 'turnId'])
    if (envelope?.[key] !== previous.binding[key]) fail('BINDING_MISMATCH');
  const wire = canonical(envelope);
  if (wire.length > 1024 * 1024) fail('EVENT_TOO_LARGE');
  const host = ['cancel', 'error', 'end'].includes(envelope.kind);
  if (!host && envelope.kind !== 'sdk') fail('UNSUPPORTED_ENVELOPE');
  const m = host ? null : envelope.message;
  if (!host) {
    string(m?.session_id, 'MISSING_SESSION');
    if (m.session_id !== previous.binding.sessionId) fail('SESSION_MISMATCH');
  }
  const id = string(host ? envelope.id : m?.uuid, 'MISSING_EVENT_ID');
  if (id.length > 1024) fail('EVENT_ID_TOO_LARGE');
  const key = (host ? 'host:' : 'sdk:') + id;
  const seen = previous.seen.find(x => x.key === key);
  if (seen) {
    if (seen.wire !== wire) fail('EVENT_ID_CONFLICT');
    return {state: structuredClone(previous), events: []};
  }
  if (previous.seen.length >= 4096) fail('EVENT_LIMIT');
  const state = structuredClone(previous), events = [];
  state.seen.push({key, wire});
  // Late messages cannot reopen or overwrite a finished turn.
  if (state.finished) return {state, events};
  const emit = body => events.push({...state.binding, sourceEventId:id, ...structuredClone(body)});
  const adverse = status => { if (state.status === 'running') state.status = status; };
  if (host) {
    adverse(envelope.kind === 'cancel' ? 'cancelled' : 'error');
    state.finished = true;
    emit({type:'terminal', status:state.status, reason:envelope.kind === 'end' ? 'incomplete_stream' : envelope.kind});
    return {state, events};
  }
  const parent = m.parent_tool_use_id ?? null;
  if (parent !== null) string(parent, 'INVALID_PARENT');
  const lane = state.stages.find(x => x.parent === parent);
  const stage = () => lane ?? fail('MISSING_STAGE');
  const messageFor = s => state.messages.find(x => x.id === s.messageId);
  if (m.type === 'stream_event') {
    const e = m.event;
    if (!e || typeof e.type !== 'string') fail('INVALID_STREAM_EVENT');
    if (e.type === 'message_start') {
      if (lane) fail('AMBIGUOUS_STAGE');
      const messageId = 'partial:' + id;
      state.stages.push({parent, messageId, blocks:[], stopped:false});
      state.messages.push({id:messageId, parentToolUseId:parent, text:'', complete:false});
    } else if (['content_block_start','content_block_delta','content_block_stop'].includes(e.type)) {
      const s = stage();
      if (s.stopped) fail('STAGE_STOPPED');
      if (!Number.isSafeInteger(e.index) || e.index < 0) fail('INVALID_BLOCK_INDEX');
      let b = s.blocks.find(x => x.index === e.index);
      if (e.type === 'content_block_stop') {
        if (b) b.stopped = true;
      } else if (e.type === 'content_block_start') {
        if (b) fail('DUPLICATE_BLOCK');
        const type = e.content_block?.type;
        if (!['text','tool_use','thinking'].includes(type)) fail('UNSUPPORTED_BLOCK');
        b = {index:e.index, type, text:'', stopped:false}; s.blocks.push(b);
        if (type === 'text') {
          if (typeof e.content_block.text !== 'string') fail('INVALID_TEXT');
          b.text = e.content_block.text;
          emit({type:'text_delta', messageId:s.messageId, index:e.index, text:b.text});
        }
      } else {
        const d = e.delta;
        if (!d || !['text_delta','input_json_delta','thinking_delta'].includes(d.type)) fail('UNSUPPORTED_DELTA');
        const type = {text_delta:'text', input_json_delta:'tool_use', thinking_delta:'thinking'}[d.type];
        const field = {text_delta:'text', input_json_delta:'partial_json', thinking_delta:'thinking'}[d.type];
        if (typeof d[field] !== 'string') fail('INVALID_DELTA');
        if (!b) { b = {index:e.index, type, text:'', stopped:false}; s.blocks.push(b); }
        if (b.stopped || b.type !== type) fail('INVALID_BLOCK_STATE');
        if (type === 'text') { b.text += d.text; emit({type:'text_delta', messageId:s.messageId, index:e.index, text:d.text}); }
        // Tool arguments/reasoning are observations only; never execute partial JSON.
        else emit({type:'progress', messageId:s.messageId, index:e.index, delta:d});
      }
      messageFor(s).text = s.blocks.slice().sort((a,b)=>a.index-b.index).filter(x=>x.type==='text').map(x=>x.text).join('');
    } else if (e.type === 'message_stop') stage().stopped = true;
    else if (e.type !== 'message_delta') fail('UNSUPPORTED_STREAM_EVENT');
  } else if (m.type === 'assistant') {
    if (!Array.isArray(m.message?.content)) fail('INVALID_CONTENT');
    const messageId = lane?.messageId ?? 'assistant:' + id;
    let text = '';
    for (const block of m.message.content) {
      if (block.type === 'text') {
        if (typeof block.text !== 'string') fail('INVALID_TEXT');
        text += block.text;
      } else if (block.type === 'tool_use') {
        const toolUseId = string(block.id,'MISSING_TOOL_ID');
        string(block.name,'MISSING_TOOL_NAME');
        if (!Object.hasOwn(block,'input')) fail('MISSING_TOOL_INPUT');
        if (state.tools.some(x=>x.id===toolUseId)) fail('TOOL_ID_CONFLICT');
        state.tools.push({id:toolUseId, name:block.name, input:structuredClone(block.input), parentToolUseId:parent, result:null});
        emit({type:'tool_call', toolUseId, name:block.name, input:block.input, parentToolUseId:parent});
      } else if (block.type !== 'thinking') fail('UNSUPPORTED_BLOCK');
    }
    const item = {id:messageId, parentToolUseId:parent, text, complete:true};
    if (lane) { Object.assign(messageFor(lane),item); state.stages=state.stages.filter(x=>x!==lane); }
    else state.messages.push(item);
    emit({type:'text_snapshot', messageId, text, complete:true});
  } else if (m.type === 'user') {
    if (!Array.isArray(m.message?.content)) fail('INVALID_CONTENT');
    for (const block of m.message.content) {
      if (block.type !== 'tool_result') fail('UNSUPPORTED_USER_CONTENT');
      const toolUseId = string(block.tool_use_id,'MISSING_TOOL_ID');
      const tool = state.tools.find(x=>x.id===toolUseId) ?? fail('UNKNOWN_TOOL');
      if (tool.result) fail('TOOL_RESULT_CONFLICT');
      if (tool.parentToolUseId !== parent) fail('TOOL_PARENT_MISMATCH');
      if (typeof block.content !== 'string' && !Array.isArray(block.content)) fail('INVALID_TOOL_RESULT');
      if (block.is_error !== undefined && typeof block.is_error !== 'boolean') fail('INVALID_TOOL_ERROR');
      tool.result = structuredClone(block);
      if (block.is_error) adverse('error');
      emit({type:'tool_result',toolUseId, content:block.content, isError:block.is_error===true, parentToolUseId:parent});
    }
  } else if (m.type === 'system') {
    if (m.subtype === 'permission_denied') {
      adverse('denied'); emit({type:'permission_denied',toolUseId:string(m.tool_use_id,'MISSING_TOOL_ID')});
    } else if (['init','status','api_retry','model_queue_status'].includes(m.subtype)) {
      emit({type:'status', subtype:m.subtype});
    } else fail('UNSUPPORTED_SYSTEM_EVENT');
  } else if (m.type === 'result') {
    if (!['success','error_max_turns','error_during_execution'].includes(m.subtype)) fail('UNSUPPORTED_RESULT');
    if (m.subtype !== 'success' || m.is_error !== false) adverse('error');
    if (m.errors !== undefined && (!Array.isArray(m.errors) || m.errors.some(x=>typeof x!=='string'))) fail('INVALID_ERRORS');
    if (m.errors?.length) adverse('error');
    if (m.permission_denials !== undefined && !Array.isArray(m.permission_denials)) fail('INVALID_DENIALS');
    if (m.permission_denials?.length) adverse('denied');
    if (state.tools.some(x=>x.result===null) || state.stages.length) adverse('error');
    if (state.status === 'running') state.status='success';
    state.finished=true;
    // Summary is metadata, NEVER another text delta/snapshot.
    emit({type:'terminal', status:state.status, summary:typeof m.result==='string'?m.result:null,
      permissionDenials:m.permission_denials??[], errors:m.errors??[], usage:m.usage??null});
  } else fail('UNSUPPORTED_MESSAGE');
  return {state, events};
}
