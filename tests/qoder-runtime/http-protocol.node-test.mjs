import './fixtures/http-offline-guard.mjs';
import {capturePath} from './fixtures/http-capture-path.mjs';
import {gzipSync} from 'node:zlib';
import assert from 'node:assert/strict';
import { test, afterEach, after } from 'node:test';
import { writeFileSync } from 'node:fs';
import { _protocol, _internal } from '../../scripts/qoder/http-protocol.mjs';

const records = [];
const RAW_MODEL = { key: 'fixture-model', display_name: 'Fixture', enable: true, source: 'fixture', max_input_tokens: 100000, is_vl: true };
const model = _internal.modelInfo(RAW_MODEL);
const TOOLS = ['fixture_read', 'fixture_stat'].map(name => ({ type: 'function', function: { name, description: 'Synthetic client-side tool', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'], additionalProperties: false } } }));
const line = body => `data: ${JSON.stringify({ headers: {}, body: typeof body === 'string' ? body : JSON.stringify(body), statusCodeValue: 200, statusCode: 'OK' })}\n\n`;
const chunk = (delta, finish_reason = null) => line({ choices: [{ index: 0, delta, finish_reason }], object: 'chat.completion.chunk' });
const done = line('[DONE]');
const call = (id = 'fixture_call_1', name = 'fixture_read', args = '{"path":"fixture.txt"}', index = 0) => ({ index, id, type: 'function', function: { name, arguments: args } });
const toolReply = (calls = [call()]) => chunk({ tool_calls: calls }) + chunk({}, 'tool_calls') + done;
const plainReply = text => chunk({ content: text }) + chunk({}, 'stop') + done;
const errorLine = (status, body) => `data: ${JSON.stringify({ statusCodeValue: status, body: JSON.stringify(body) })}\n\n`;
const frames = text => text.split('\n').filter(x => x.startsWith('data: ')).map(x => x.slice(6)).map(x => x === '[DONE]' ? x : JSON.parse(x));
const chat = (more = {}) => ({ model: 'fixture-model', messages: [{ role: 'user', content: 'Fixture request only.' }], tools: structuredClone(TOOLS), ...more });
const stable = body => { const result = structuredClone(body); delete result.business; return result; };
const noFinish = fs => { assert.ok(!fs.some(f => f.choices?.[0]?.finish_reason)); assert.ok(!fs.includes('[DONE]')); };
const noTools = fs => assert.ok(!fs.some(f => f.choices?.[0]?.delta?.tool_calls?.length));

afterEach(() => assert.deepEqual(globalThis.__offlineFixtureAttempts, []));
after(() => writeFileSync(capturePath('fixed-contract-captures.json.gz'), gzipSync(JSON.stringify({
  scope: 'Isolated pure protocol candidate: direct qoderBody/answer calls only; no auth, refresh, account, fetch, SDK, Rust, or integration wiring.',
  forbiddenAttempts: globalThis.__offlineFixtureAttempts,
  records,
}, null, 2))));

async function consume(res) {
  const out = { status: res.status, headers: Object.fromEntries(res.headers), body: '', consumedChunks: [] };
  const reader = res.body.getReader(), decoder = new TextDecoder();
  for (;;) {
    const next = await reader.read();
    if (next.done) break;
    const text = decoder.decode(next.value, { stream: true });
    out.consumedChunks.push(text); out.body += text;
  }
  out.body += decoder.decode();
  return { ...out, json: () => JSON.parse(out.body), frames: () => frames(out.body) };
}
async function session(label, replies) {
  const record = { label, synthetic: true, requests: [], responses: [] };
  records.push(record);
  let i = 0;
  return { record, posted: () => record.requests.map(r => r.decodedBody), ask: async request => {
    assert.ok(i < replies.length, 'No implicit additional request');
    const reply = replies[i++];
    record.requests.push({ request, decodedBody: _internal.qoderBody(request, model), syntheticResponse: typeof reply === 'string' ? reply : '[synthetic stream]' });
    const res = await _protocol.answer(typeof reply === 'function' ? reply() : new Response(reply, { headers: { 'Content-Type': 'text/event-stream' } }), request);
    const out = await consume(res);
    record.responses.push(out);
    return out;
  } };
}
async function rejectsTool(label, reply, request = chat(), expectedStatus = 502) {
  const s = await session(label, [reply]);
  const out = await s.ask(request);
  assert.equal(out.status, expectedStatus);
  assert.ok(out.json().error);
  assert.equal(out.json().choices, undefined);
  assert.equal(s.record.requests.length, 1);
}

test('1. no-execute: native tool result is complete; direct protocol path performs no fetch, auth or process action', async () => {
  const s = await session('fixed-client-no-execute', [toolReply()]);
  const out = await s.ask(chat());
  assert.equal(out.status, 200);
  assert.equal(out.json().choices[0].finish_reason, 'tool_calls');
  assert.deepEqual(out.json().choices[0].message.tool_calls, [{ id: 'fixture_call_1', type: 'function', function: { name: 'fixture_read', arguments: '{"path":"fixture.txt"}' } }]);
  assert.equal(s.record.requests.length, 1);
  assert.deepEqual(globalThis.__offlineFixtureAttempts, []);
});

test('2. full supported history still round-trips tool IDs, results, raw schemas and prior turns in independent bodies', async () => {
  const calls = [call('fixture_read_id'), call('fixture_stat_id', 'fixture_stat', '{"path":"fixture.txt"}', 1)];
  const s = await session('fixed-two-round-history', [toolReply(calls), plainReply('Finished.')]);
  const base = [{ role: 'system', content: 'System fixture.' }, { role: 'developer', content: 'Developer fixture.' }, { role: 'user', content: 'Prior user.' }, { role: 'assistant', content: 'Prior assistant.' }, { role: 'user', content: 'Read and stat.' }];
  const one = await s.ask(chat({ messages: base }));
  const assistant = one.json().choices[0].message;
  const messages = [...base, assistant, { role: 'tool', tool_call_id: 'fixture_read_id', content: 'Fixture content.' }, { role: 'tool', tool_call_id: 'fixture_stat_id', content: '{"size":23}' }, { role: 'user', content: 'Summarize.' }];
  const two = await s.ask(chat({ messages }));
  assert.equal(two.json().choices[0].message.content, 'Finished.');
  const [a, b] = s.posted();
  assert.notEqual(a.business.id, b.business.id);
  assert.deepEqual(b.tools, TOOLS);
  assert.deepEqual(b.messages.slice(4, 7), [assistant, messages[6], messages[7]]);
  assert.match(b.system[0].text, /System fixture\.\n\nDeveloper fixture\./);
  assert.equal(b.messages[1].content[0].text, 'Prior user.');
  assert.equal(b.messages[2].content[0].text, 'Prior assistant.');
  assert.equal(b.messages.at(-1).content[0].text, 'Summarize.');
});

test('3. tool image history conversion remains unchanged', async () => {
  const s = await session('fixed-tool-image-history', [plainReply('Seen.')]);
  const im = { type: 'image_url', image_url: { url: 'data:image/png;base64,AA==' } };
  await s.ask(chat({ messages: [{ role: 'user', content: 'Read.' }, { role: 'assistant', content: '', tool_calls: [{ id: 'image_call', type: 'function', function: { name: 'fixture_read', arguments: '{"path":"fixture.png"}' } }] }, { role: 'tool', tool_call_id: 'image_call', content: [{ type: 'text', text: 'Image.' }, im] }, { role: 'user', content: 'Describe.' }] }));
  const b = s.posted()[0];
  assert.equal(b.messages[3].tool_call_id, 'image_call');
  assert.deepEqual(b.messages[4].content.slice(1), [im, { type: 'text', text: 'Describe.' }]);
});

test('4. named selection and parallel=false reject wrong/multiple tools; raw outbound schema is unchanged', async () => {
  const named = chat({ tool_choice: { type: 'function', function: { name: 'fixture_read' } } });
  await rejectsTool('fixed-named-wrong', toolReply([call('wrong', 'fixture_stat')]), named);
  await rejectsTool('fixed-parallel-forbidden', toolReply([call(), call('second', 'fixture_stat', '{}', 1)]), chat({ parallel_tool_calls: false }));
  const s = await session('fixed-named-control', [toolReply()]);
  assert.equal((await s.ask(named)).status, 200);
  assert.deepEqual(stable(s.posted()[0]), stable(_internal.qoderBody(chat(), model))); // selection enforcement is final-response validation, not a new wire field
});

test('5. none rejects returned tool calls without leaking any call', async () => {
  await rejectsTool('fixed-none', toolReply(), chat({ tool_choice: 'none', stream: true }));
  assert.equal(_internal.qoderBody(chat({ tool_choice: 'none' }), model).tools, undefined);
});

test('6. required rejects a final plain response and accepts a valid declared tool', async () => {
  await rejectsTool('fixed-required-plain', plainReply('No tool.'), chat({ tool_choice: 'required' }));
  const s = await session('fixed-required-control', [toolReply()]);
  assert.equal((await s.ask(chat({ tool_choice: 'required' }))).status, 200);
});

test('7. unknown tool names are rejected; arbitrary JSON Schema validation is explicitly not claimed', async () => {
  await rejectsTool('fixed-unknown-tool', toolReply([call('unknown_id', 'not_declared_tool', '{"unexpected":true}')]));
  const s = await session('known-tool-schema-not-validated', [toolReply([call('known_id', 'fixture_read', '{"unexpected":true}')])]);
  assert.equal((await s.ask(chat())).status, 200); // object completeness is checked, schema-required path is not
});

test('8. text before upstream error remains streamed, followed by error and no finish or DONE', async () => {
  const s = await session('fixed-stream-error-after-text', [chunk({ content: 'Visible prefix.' }) + errorLine(403, { code: 10605, message: 'fixture queue' })]);
  const out = await s.ask(chat({ stream: true }));
  assert.equal(out.status, 200);
  assert.ok(out.frames().some(f => f.choices?.[0]?.delta?.content === 'Visible prefix.'));
  assert.equal(out.frames().at(-1).error.code, 503);
  noFinish(out.frames());
});

test('9. nonstream upstream error after text remains a 503 with no completion choices', async () => {
  await rejectsTool('fixed-nonstream-error-after-text', chunk({ content: 'Buffered prefix.' }) + errorLine(403, { code: 10605, message: 'fixture queue' }), chat(), 503);
});

test('10. malformed later tool never allows an earlier buffered tool to escape', async () => {
  const broken = chunk({ tool_calls: [call()] }) + chunk({ content: '<tool_call>{"name":"fixture_read","arguments":{broken}}</tool_call>' });
  await rejectsTool('fixed-error-after-buffered-tool', broken, chat({ stream: true }));
  const s = await session('fixed-text-before-buffered-tool-error', [chunk({ content: 'Visible prefix.' }) + broken]);
  const out = await s.ask(chat({ stream: true }));
  assert.equal(out.status, 200);
  noTools(out.frames()); noFinish(out.frames());
  assert.equal(out.frames().at(-1).error.code, 502);
});

test('11. transport failure after output now becomes an error frame, preserving prefix without success', async () => {
  const enc = new TextEncoder();
  const s = await session('fixed-transport-error-after-output', [() => {
    let n = 0;
    return new Response(new ReadableStream({ pull(ctl) { if (n++ === 0) ctl.enqueue(enc.encode(chunk({ content: 'Prefix before disconnect.' }))); else ctl.error(new Error('SYNTHETIC_CONNECTION_RESET')); } }));
  }]);
  const out = await s.ask(chat({ stream: true }));
  assert.equal(out.status, 200);
  assert.match(out.body, /Prefix before disconnect/);
  assert.equal(out.frames().at(-1).error.code, 502);
  noFinish(out.frames());
});

test('12. empty, malformed outer/inner, and DONE-only SSE are errors', async () => {
  for (const reply of ['', 'data: not-json\n\n', line('{not-json}'), done]) await rejectsTool('fixed-empty-or-malformed-sse', reply);
});

test('13. native truncated or nonobject JSON is rejected without tool leakage', async () => {
  for (const args of ['{"path":', '[]', 'null', '42', '"string"']) await rejectsTool('fixed-native-invalid-json', toolReply([call('invalid_id', 'fixture_read', args)]), chat({ stream: true }));
});

test('14. interleaved native indexes assemble into the two original calls', async () => {
  const reply = chunk({ tool_calls: [call('a', 'fixture_read', '{"path":', 0), call('b', 'fixture_stat', '{"path":', 1)] }) + chunk({ tool_calls: [{ index: 0, function: { arguments: '"a"}' } }, { index: 1, function: { arguments: '"b"}' } }] }) + chunk({}, 'tool_calls') + done;
  const s = await session('fixed-interleaved-native-fragments', [reply]);
  const out = await s.ask(chat());
  const calls = out.json().choices[0].message.tool_calls;
  assert.deepEqual(calls.map(c => [c.id, c.function.name, JSON.parse(c.function.arguments)]), [['a', 'fixture_read', { path: 'a' }], ['b', 'fixture_stat', { path: 'b' }]]);
});

test('valid XML/JSON text-call Splitter behavior survives whole, fragmented and closing-tag-omitted cases', async () => {
  for (const content of [
    'Reading.\n<tool_call>{"name":"fixture_read","arguments":{"path":"a.go"}}</tool_call>',
    'Reading.\n<tool_call>{"name":"fixture_read","arguments":{"path":"a.go"}}',
    'Reading.\n<tool_call><function=fixture_read><parameter=path>a.go</parameter></function>',
  ]) {
    for (const reply of [chunk({ content }) + done, Array.from({ length: Math.ceil(content.length / 3) }, (_, k) => chunk({ content: content.slice(k * 3, k * 3 + 3) })).join('') + done]) {
      const s = await session('fixed-original-splitter-control', [reply]);
      const out = await s.ask(chat({ stream: true }));
      assert.equal(out.status, 200);
      const fs = out.frames();
      assert.equal(fs.filter(f => f.choices?.[0]?.delta?.content).map(f => f.choices[0].delta.content).join(''), 'Reading.\n');
      const tools = fs.flatMap(f => f.choices?.[0]?.delta?.tool_calls ?? []);
      assert.equal(tools.length, 1);
      assert.equal(tools[0].function.name, 'fixture_read');
      assert.deepEqual(JSON.parse(tools[0].function.arguments), { path: 'a.go' });
      assert.equal(fs.at(-1), '[DONE]');
    }
  }
});

test('original malformed text/XML call protections still reject with a supplied valid terminal', async () => {
  for (const content of ['<tool_call>{"name":"fixture_read","arguments":{"path":}}</tool_call>', '<tool_call>{"name":"fixture_read","arguments":{"path":"a.go"', '<tool_call><function=><parameter=path>a.go</parameter></function></tool_call>']) {
    await rejectsTool('fixed-original-splitter-negative', chunk({ content }) + done);
  }
});

test('usage tail survives finish_reason and optional DONE; DONE is a valid terminal after content', async () => {
  const usage = line({ choices: [], usage: { prompt_tokens: 10, completion_tokens: 3, prompt_tokens_details: { cached_tokens: 4 }, completion_tokens_details: { reasoning_tokens: 1 } } });
  for (const reply of [chunk({ content: 'Text.' }) + chunk({}, 'stop') + usage + done, chunk({ content: 'Text.' }) + chunk({}, 'stop') + usage]) {
    const s = await session('fixed-usage-tail', [reply]);
    const out = await s.ask(chat());
    assert.equal(out.status, 200);
    assert.deepEqual(out.json().usage, { prompt_tokens: 10, completion_tokens: 3, total_tokens: 13, prompt_tokens_details: { cached_tokens: 4 }, completion_tokens_details: { reasoning_tokens: 1 } });
  }
  const s = await session('fixed-done-without-finish', [chunk({ content: 'Text.' }) + done]);
  assert.equal((await s.ask(chat())).status, 200);
});

test('missing terminal, unsupported finish or malformed tail cannot fabricate success', async () => {
  for (const reply of [chunk({ content: 'Partial.' }), chunk({ content: 'Partial.' }) + chunk({}, 'unsupported'), chunk({ content: 'Partial.' }) + chunk({}, 'stop') + 'data: bad\n\n']) {
    const s = await session('fixed-terminal-negative', [reply]);
    const out = await s.ask(chat({ stream: true }));
    assert.equal(out.status, 200);
    assert.equal(out.frames().at(-1).error.code, 502);
    noFinish(out.frames());
  }
});

test('unknown tool_choice forms fail explicitly before output', async () => {
  for (const choice of ['invalid', null, true, [], {}, { type: 'unknown' }, { type: 'function', function: { name: 'not_declared' } }]) {
    await rejectsTool('fixed-invalid-tool-choice', plainReply('Text.'), chat({ tool_choice: choice, stream: true }), 400);
  }
});

test('native missing/conflicting IDs and invalid indexes are rejected', async () => {
  await rejectsTool('fixed-native-missing-id', toolReply([{ index: 0, function: { name: 'fixture_read', arguments: '{}' } }]));
  await rejectsTool('fixed-native-duplicate-id', toolReply([call('same'), call('same', 'fixture_stat', '{}', 1)]));
  await rejectsTool('fixed-native-invalid-index', toolReply([call('a', 'fixture_read', '{}', -1)]));
  await rejectsTool('fixed-native-conflicting-id', chunk({ tool_calls: [call('a', 'fixture_read', '{', 0)] }) + chunk({ tool_calls: [call('b', 'fixture_read', '}', 0)] }) + done);
});

test('tool buffer count, per-call bytes and cumulative bytes are bounded', async () => {
  await rejectsTool('fixed-count-bound', toolReply(Array.from({ length: 65 }, (_, i) => call('id_' + i, 'fixture_read', '{}', i))));
  await rejectsTool('fixed-per-call-byte-bound', toolReply([call('large', 'fixture_read', JSON.stringify({ path: 'x'.repeat(256 * 1024) }))]));
  await rejectsTool('fixed-total-byte-bound', toolReply(Array.from({ length: 5 }, (_, i) => call('big_' + i, 'fixture_read', JSON.stringify({ path: 'x'.repeat(220 * 1024) }), i))));
});

test('text is observable before terminal while tools remain buffered until validation', { timeout: 1500 }, async () => {
  let control;
  const enc = new TextEncoder();
  const upstream = new ReadableStream({ start(ctl) { control = ctl; ctl.enqueue(enc.encode(chunk({ content: 'Immediate text.' }) + chunk({ tool_calls: [call()] }))); } });
  const response = await _protocol.answer(new Response(upstream), chat({ stream: true }));
  assert.equal(response.status, 200);
  const reader = response.body.getReader(), decoder = new TextDecoder();
  const role = decoder.decode((await reader.read()).value);
  const text = decoder.decode((await reader.read()).value);
  assert.match(role, /assistant/);
  assert.match(text, /Immediate text/);
  let settled = false;
  const pending = reader.read().then(value => { settled = true; return value; });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(settled, false);
  control.enqueue(enc.encode(chunk({}, 'tool_calls') + done)); control.close();
  const tool = decoder.decode((await pending).value);
  assert.match(tool, /fixture_call_1/);
  let tail = '';
  for (;;) { const next = await reader.read(); if (next.done) break; tail += decoder.decode(next.value); }
  assert.match(tail, /tool_calls/);
  assert.match(tail, /\[DONE\]/);
});
