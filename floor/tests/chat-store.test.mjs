// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';
import test, { beforeEach } from 'node:test';
import { applyServerFrame, beginFloorEpoch, getFloorState, invalidateFloorEpoch, noteTransport,
  resetFloorStore, selectOutputAgent } from '../src/renderer/floor/store.ts';

const A = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const G = '01ARZ3NDEKTSV4RRFFQ69G5FAZ';
const empty = () => ({ enabled: true, max_text_bytes: 4096, pending: [], turns: [], context_sent: [] });
const populated = () => ({ ...empty(), pending: [{ agent_id: A, note_lsn: '7', goal_id: G, text: 'Retained note',
  state: 'queued', turn_id: null, cancellable: true, reason: null }] });
const snapshot = (chat, lsn = 9) => JSON.stringify({ type: 'floor.snapshot',
  projection: { lsn, agents: [{ id: A, name: 'Ada', mode: 'managed' }] }, ...(chat === undefined ? {} : { chat }) });
const delta = (chat, lsn = 10) => JSON.stringify({ type: 'floor.delta', delta: { lsn }, ...(chat === undefined ? {} : { chat }) });

beforeEach(resetFloorStore);

test('a snapshot exposes Chat only when its exact enabled sibling is valid', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame(snapshot(populated()), epoch);
  assert.deepEqual(getFloorState().chat, populated());
  applyServerFrame(snapshot(undefined), epoch);
  assert.equal(getFloorState().chat, null);
  applyServerFrame(snapshot({ ...populated(), provider_error: 'Private' }), epoch);
  assert.equal(getFloorState().chat, null);
  assert.equal(getFloorState().rejected, 1);
});

test('delta omission retains Chat and a present replacement empties every array', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame(snapshot(populated()), epoch);
  applyServerFrame(delta(undefined), epoch);
  assert.equal(getFloorState().chat.pending[0].text, 'Retained note');
  applyServerFrame(delta(empty(), 11), epoch);
  assert.deepEqual(getFloorState().chat, empty());
});

test('an authenticated post-ack Chat replacement applies at the same projection position without admitting older positions', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame(snapshot(populated()), epoch);
  applyServerFrame('{"type":"floor.delta","delta":{"lsn":10}}', epoch);
  assert.equal(getFloorState().chat.pending[0].text, 'Retained note');
  applyServerFrame('{"type":"floor.delta","delta":{"lsn":10},"chat":{"enabled":true,"max_text_bytes":4096,"pending":[],"turns":[],"context_sent":[]}}', epoch);
  assert.deepEqual(getFloorState().chat, empty());
  applyServerFrame(delta(populated(), 9), epoch);
  assert.deepEqual(getFloorState().chat, empty(), 'older projection position restored Chat');
  selectOutputAgent(A);
  applyServerFrame(delta(populated(), 10), epoch);
  assert.deepEqual(getFloorState().chat, populated(), 'selection blocked a current global Chat replacement');
  invalidateFloorEpoch();
  applyServerFrame(delta(populated(), 10), epoch);
  assert.equal(getFloorState().chat, null, 'old connection restored same-position Chat');
});

test('disconnect, resync, invalidation and a new connection clear Chat before any new snapshot', () => {
  for (const clear of [() => noteTransport('close'), () => invalidateFloorEpoch(), () => beginFloorEpoch(),
    epoch => applyServerFrame('{"type":"disconnected"}', epoch),
    epoch => applyServerFrame('{"type":"floor.resync"}', epoch)]) {
    resetFloorStore();
    const epoch = beginFloorEpoch();
    applyServerFrame(snapshot(populated()), epoch);
    assert.equal(getFloorState().chat.pending.length, 1);
    clear(epoch);
    assert.equal(getFloorState().chat, null);
    applyServerFrame(snapshot(populated()), epoch);
    assert.equal(getFloorState().chat, null, 'old epoch restored Chat');
  }
});

test('output selection and retry retain global Chat and accept current replacement deltas', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame(snapshot(populated()), epoch);
  selectOutputAgent(A);
  assert.deepEqual(getFloorState().chat, populated());
  applyServerFrame(delta(empty()), epoch);
  assert.deepEqual(getFloorState().chat, empty());
  selectOutputAgent(A, true);
  assert.deepEqual(getFloorState().chat, empty());
  assert.equal(getFloorState().epoch, epoch);
  applyServerFrame(snapshot(populated(), 11), epoch);
  assert.deepEqual(getFloorState().chat, populated());
});

test('a malformed replacement rejects the whole Chat sibling without damaging ordinary rows', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame(snapshot(populated()), epoch);
  applyServerFrame(delta({ ...populated(), turns: [{ agent_id: A, text: 'Unrelated' }] }), epoch);
  assert.equal(getFloorState().chat, null);
  assert.equal(getFloorState().agents[0].name, 'Ada');
  assert.equal(getFloorState().rejected, 1);
});

test('a literal snapshot with an unmatched reserved turn rejects Chat and preserves the ordinary agent', () => {
  const epoch = beginFloorEpoch();
  applyServerFrame('{"type":"floor.snapshot","projection":{"lsn":9,"agents":[{"id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","name":"Ada","mode":"managed"}]},"chat":{"enabled":true,"max_text_bytes":4096,"pending":[],"turns":[{"agent_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","turn_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","note_lsn":"7","text":"Reserved note","state":"reserved","reply":null,"truncated":null,"reissued":false}],"context_sent":[]}}', epoch);
  assert.equal(getFloorState().chat, null);
  assert.equal(getFloorState().agents[0].name, 'Ada');
  assert.equal(getFloorState().rejected, 1);
  applyServerFrame(snapshot(populated()), epoch);
  assert.equal(getFloorState().chat.pending[0].text, 'Retained note');
});
