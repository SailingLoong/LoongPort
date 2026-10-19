import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { runWorker } from '../../../scripts/qoder/worker.mjs';
import { startQoderTask } from '../../../scripts/qoder/task-entry.mjs';
const root = process.argv[process.argv.indexOf('--sdk-root') + 1];
await runWorker({ startTask: request => {
  const account = request.binding.accountId;
  appendFileSync(join(root, 'attempts.jsonl'), JSON.stringify({ account }) + '\n');
  return startQoderTask(request, { loadSdk: async () => ({ query: ({ prompt, options }) => ({
    async *[Symbol.asyncIterator]() {
      if (options.auth.accessToken !== `synthetic:${account}`) throw Error('synthetic auth binding mismatch');
      if (account === 'qoder-unknown') throw Error('synthetic unknown SDK failure');
      const history = JSON.parse(prompt.slice(prompt.indexOf('\n') + 1));
      if (history.length > 1) {
        const expected = [
          ['system', 'leading system'], ['user', 'first'], ['developer', 'middle developer'],
          ['assistant', 'prior'], ['system', 'middle system'], ['user', 'second'],
        ];
        if (JSON.stringify(history.map(m => [m.role, m.content])) !== JSON.stringify(expected)) throw Error('synthetic history reordered');
      }
      yield { type: 'assistant', uuid: 'a', session_id: options.sessionId,
        message: { content: [{ type: 'text', text: account === 'qoder-partial' ? 'partial-before-failure' : 'ordered-history-and-auth-ok' }] } };
      if (account === 'qoder-partial') throw Error('synthetic failure after output');
      yield { type: 'result', uuid: 'r', session_id: options.sessionId, subtype: 'success', is_error: false };
    }, async close() {},
  }) }) });
} });
