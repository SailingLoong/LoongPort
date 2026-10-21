// Test capture output belongs to a unique system temporary directory, never the repository.
import {mkdtempSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
let directory;
export function capturePath(name) {
  directory ??= mkdtempSync(join(tmpdir(), 'loongport-qoder-http-test-captures-'));
  return join(directory, name);
}
