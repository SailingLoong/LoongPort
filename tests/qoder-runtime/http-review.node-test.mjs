import './fixtures/http-offline-guard.mjs';
import {capturePath} from './fixtures/http-capture-path.mjs';
import {gzipSync} from 'node:zlib';
import assert from 'node:assert/strict';
import { test, afterEach, after } from 'node:test';
import { writeFileSync } from 'node:fs';
import { _protocol } from '../../scripts/qoder/http-protocol.mjs';
const records = [];
const line = body => `data: ${JSON.stringify({ body: typeof body === 'string' ? body : JSON.stringify(body), statusCodeValue: 200 })}\n\n`;
const done = line('[DONE]');
const chat = { model: 'fixture-model', messages: [{ role: 'user', content: 'fixture' }], tools: [{ type: 'function', function: { name: 'fixture_read', parameters: { type: 'object' } } }] };
const tc = { index: 0, id: 'fixture_id', type: 'function', function: { name: 'fixture_read', arguments: '{}' } };
async function ask(label, choices) {
  const input = line({ choices }) + done;
  const res = await _protocol.answer(new Response(input), chat);
  const body = await res.json();
  records.push({ label, input, status: res.status, body });
  return { status: res.status, body };
}
afterEach(() => assert.deepEqual(globalThis.__offlineFixtureAttempts, []));
after(() => writeFileSync(capturePath('review-regression-captures.json.gz'), gzipSync(JSON.stringify(records, null, 2))));

test('review: explicit non-function native type cannot be rewritten into a successful function call', async () => {
  for (const type of ['custom', null]) {
    const out = await ask('native-type-' + String(type), [{ index: 0, delta: { tool_calls: [{ ...tc, type }] }, finish_reason: 'tool_calls' }]);
    assert.equal(out.status, 502);
    assert.equal(out.body.choices, undefined);
    assert.match(out.body.error.message, /native tool type/);
  }
  const control = await ask('native-function-control', [{ index: 0, delta: { tool_calls: [tc] }, finish_reason: 'tool_calls' }]);
  assert.equal(control.status, 200);
});

test('review: explicit nonzero choice index is rejected while missing or zero index is accepted', async () => {
  for (const index of [7, -1, null, '0']) {
    const out = await ask('choice-index-' + String(index), [{ index, delta: { content: 'Fixture text.' }, finish_reason: 'stop' }]);
    assert.equal(out.status, 502);
    assert.equal(out.body.choices, undefined);
    assert.match(out.body.error.message, /choice index/);
  }
  for (const shape of [{ index: 0 }, {}]) {
    const out = await ask('choice-index-control', [{ ...shape, delta: { content: 'Fixture text.' }, finish_reason: 'stop' }]);
    assert.equal(out.status, 200);
    assert.equal(out.body.choices[0].message.content, 'Fixture text.');
  }
});

test('terminal wire: oversized usage cannot split a finish frame or emit successful DONE', async () => {
  const input = line({ choices: [{ index: 0, delta: { content: 'visible-prefix' }, finish_reason: 'stop' }], usage: { prompt_tokens: 'x'.repeat(60 * 1024), completion_tokens: 1, total_tokens: 1 } }) + done;
  let aborted = 0;
  const response = await _protocol.answer(new Response(input), { ...chat, stream: true }, { abortUpstream: () => { aborted += 1; } });
  const reader = response.body.getReader();
  let text = '', failed = false;
  try { for (;;) { const result = await reader.read(); if (result.done) break; text += new TextDecoder().decode(result.value); } } catch { failed = true; }
  assert.equal(failed, true);
  assert.ok(text.includes('visible-prefix'));
  assert.ok(!text.includes('"finish_reason":"stop"'));
  assert.ok(!text.includes('data: [DONE]'));
  assert.ok(aborted > 0);
});
