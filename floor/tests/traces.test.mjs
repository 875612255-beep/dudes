// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import test, { after, before, beforeEach } from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const root = resolve(import.meta.dirname, '..');
const ADA = '01JBQ8XK2M3N4P5Q6R7S8T9VWX';
const SECRET = 'private-trace-sentinel-DO-NOT-DISPLAY';
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
const id = (index) => `${ADA.slice(0, 24)}${ALPHABET[Math.floor(index / 32)]}${ALPHABET[index % 32]}`;
const trace = (at = 1_787_394_600_000) => ({ agent_id: ADA, started_at: at,
  duration_ms: 250, outcome: 'ended', engine: 'claude', tokens: '42', cost_usd_micros: '31575' });
const agent = { id: ADA, name: 'Ada', role: 'engineer', department: 'engineering',
  provider: 'claude', status: 'idle', mode: 'managed' };
let server;
let store;
let AgentDetail;

before(async () => {
  server = await createServer({ root, cacheDir: resolve(root, 'node_modules/.vite-traces'),
    server: { middlewareMode: true, hmr: false }, appType: 'custom', logLevel: 'silent' });
  store = await server.ssrLoadModule('/src/renderer/floor/store.ts');
  ({ AgentDetail } = await server.ssrLoadModule('/src/renderer/AgentDetail.tsx'));
});
after(async () => { await server?.close(); });
beforeEach(() => store.resetFloorStore());

function snapshot(rows) {
  const epoch = store.beginFloorEpoch();
  store.noteTransport('open');
  store.applyServerFrame(JSON.stringify({ type: 'floor.snapshot',
    projection: { lsn: 10, agents: [agent], cards: [] }, traces: rows }), epoch);
  return epoch;
}
function detail() {
  return renderToStaticMarkup(createElement(AgentDetail, { row: agent,
    traces: store.getFloorState().traces }));
}

test('the exact trace allowlist reaches the detail and rejects a planted secret', () => {
  snapshot([trace()]);
  assert.deepEqual(store.getFloorState().traces, [trace()]);
  assert.match(detail(), /aria-label="Traces"/);
  assert.match(detail(), /<p>Up to 8 recent turns, fewer when rows are long\.<\/p>/);
  assert.match(detail(), /42 tokens/);
  assert.match(detail(), /\$0\.031575/);
  store.resetFloorStore();
  snapshot([{ ...trace(), last_message: SECRET }]);
  assert.deepEqual(store.getFloorState().traces, []);
  assert.equal(store.getFloorState().rejected, 1);
  assert.ok(!detail().includes(SECRET));
});

test('trace bounds and complete replacement follow the Floor epoch', () => {
  const epoch = snapshot([trace()]);
  assert.equal(store.getFloorState().traces.length, 1);
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 11, agents: [] },
    traces: [{ ...trace(1_787_394_601_000), outcome: 'held' }] }), epoch);
  assert.equal(store.getFloorState().traces[0].outcome, 'held');
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 12, agents: [] } }), epoch);
  assert.equal(store.getFloorState().traces[0].outcome, 'held');
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 13, agents: [] }, traces: [] }), epoch);
  assert.deepEqual(store.getFloorState().traces, []);
  store.resetFloorStore();
  snapshot(Array.from({ length: 65 }, (_, i) => trace(i)));
  assert.deepEqual(store.getFloorState().traces, []);
  assert.ok(store.getFloorState().rejected > 0);
  store.resetFloorStore();
  snapshot(Array.from({ length: 9 }, (_, i) => ({ ...trace(i), tokens: null, cost_usd_micros: null })));
  assert.deepEqual(store.getFloorState().traces, []);
  store.resetFloorStore();
  snapshot(Array.from({ length: 65 }, (_, i) => ({ ...trace(i), agent_id: id(i) })));
  assert.deepEqual(store.getFloorState().traces, []);
  store.resetFloorStore();
  const max = { ...trace(), duration_ms: Number.MAX_SAFE_INTEGER,
    engine: 'antigravity', outcome: 'failed', tokens: '18446744073709551615',
    cost_usd_micros: '18446744073709551615' };
  snapshot(Array.from({ length: 64 }, (_, i) => ({ ...max, agent_id: id(Math.floor(i / 4)) })));
  assert.deepEqual(store.getFloorState().traces, []);
  store.resetFloorStore();
  snapshot([{ ...trace(), tokens: '0', cost_usd_micros: null, outcome: 'failed' }]);
  assert.match(detail(), /failed/);
  store.noteTransport('close');
  assert.deepEqual(store.getFloorState().traces, []);
});

test('resync and an old stream epoch cannot restore traces', () => {
  const epoch = snapshot([trace()]);
  assert.equal(store.getFloorState().traces.length, 1);
  store.applyServerFrame(JSON.stringify({ type: 'floor.resync' }), epoch);
  assert.deepEqual(store.getFloorState().traces, []);
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 11, agents: [] },
    traces: [trace()] }), epoch);
  assert.deepEqual(store.getFloorState().traces, []);
  const fresh = snapshot([trace()]);
  assert.equal(store.getFloorState().traces.length, 1);
  store.invalidateFloorEpoch();
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 12, agents: [] },
    traces: [trace()] }), fresh);
  assert.deepEqual(store.getFloorState().traces, []);
});
