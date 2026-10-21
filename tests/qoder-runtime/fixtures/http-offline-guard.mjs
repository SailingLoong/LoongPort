// Offline test guard, not a production implementation. All request bodies are synthetic.
import cp from 'node:child_process';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import tls from 'node:tls';
import dgram from 'node:dgram';
import { syncBuiltinESMExports } from 'node:module';
const attempts = [];
const deny = (kind) => (...args) => {
  attempts.push({ kind });
  throw new Error(`OFFLINE_FIXTURE_DENIED: ${kind}`);
};
for (const key of ['exec', 'execFile', 'execSync', 'execFileSync', 'spawn', 'spawnSync', 'fork']) cp[key] = deny('child_process.' + key);
for (const [mod, name, keys] of [[http, 'http', ['request', 'get']], [https, 'https', ['request', 'get']], [net, 'net', ['connect', 'createConnection']], [tls, 'tls', ['connect']], [dgram, 'dgram', ['createSocket']]]) {
  for (const key of keys) mod[key] = deny(name + '.' + key);
}
net.Socket.prototype.connect = deny('net.Socket.connect');
syncBuiltinESMExports();
globalThis.fetch = deny('unmocked_fetch');
globalThis.__offlineFixtureAttempts = attempts;
