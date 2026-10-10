// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Firing control for this file: substitute one displayed value — change the
// expected role, status, goal or cost in `EXPECTED` and "every field of a row
// reaches the fields it publishes" goes red naming the agent and the field.

import assert from 'node:assert/strict';
import test from 'node:test';

import { fieldsOf, formatCost, formatUsd } from '../src/renderer/scene/label.ts';

// Two deliberately distinct rows: no two fields share a value, so a reading
// of the wrong row, the wrong field or a constant cannot pass by luck.
const ROWS = [
  {
    id: '01J8ZP0000000000000000000A',
    role: 'backend engineer',
    status: 'running',
    goal: { id: 'g-1', text: 'ship the parser', status: 'in_progress' },
    spent_today: { usd: 1_499_999, tokens: 12_345 },
  },
  {
    id: '01J8ZP0000000000000000000B',
    role: 'frontend engineer',
    status: 'exhausted',
    goal: { id: 'g-2', text: 'draw the floor', status: 'set' },
    spent_today: { usd: 20_000, tokens: 7 },
  },
];

const EXPECTED = {
  '01J8ZP0000000000000000000A': {
    role: 'backend engineer', status: 'running', goal: 'ship the parser', goalStatus: 'in_progress',
    cost: '$1.49 · 12345 tok', usd: '$1.49',
  },
  '01J8ZP0000000000000000000B': {
    role: 'frontend engineer', status: 'exhausted', goal: 'draw the floor', goalStatus: 'set',
    cost: '$0.02 · 7 tok', usd: '$0.02',
  },
};

test('dollars come from micro-dollars by integer arithmetic, truncated', () => {
  assert.equal(formatUsd(0), '$0.00');
  assert.equal(formatUsd(9_999), '$0.00', 'less than a cent is not a cent');
  assert.equal(formatUsd(10_000), '$0.01');
  assert.equal(formatUsd(1_499_999), '$1.49', 'truncated, never rounded up');
  assert.equal(formatUsd(2_000_000), '$2.00');
  assert.equal(formatUsd(123_456_789), '$123.45');
  // The wire defaults these fields, so an absent one is zero, not a blank.
  assert.equal(formatUsd(undefined), '$0.00');
  assert.equal(formatUsd(-5), '$0.00');
});

test('cost carries both halves of spent_today', () => {
  assert.equal(formatCost({ usd: 1_499_999, tokens: 12_345 }), '$1.49 · 12345 tok');
  assert.equal(formatCost({}), '$0.00 · 0 tok');
  assert.equal(formatCost(undefined), '$0.00 · 0 tok');
});

test('every field of a row reaches the fields it publishes', () => {
  for (const row of ROWS) {
    const fields = fieldsOf(row);
    for (const key of ['role', 'status', 'goal', 'goalStatus', 'cost', 'usd']) {
      assert.equal(fields[key], EXPECTED[row.id][key], `${row.id} ${key}`);
    }
  }
});

test('a waiting agent shows its question, byte for byte', () => {
  // A nonce, so a question that arrived by any route other than this row's own field cannot match by accident.
  const question = 'Ship 7f3a9c1e or wait for review?';
  const fields = fieldsOf({ ...ROWS[0], status: 'waiting', question });
  assert.equal(fields.question, question);
  assert.equal(fields.status, 'waiting');
});

test('a waiting agent with no question shows no invented text', () => {
  for (const question of [undefined, '', null, 42]) {
    assert.equal(fieldsOf({ ...ROWS[0], status: 'waiting', question }).question, '', `${String(question)} became a question`);
  }
});

test('an agent that is not waiting does not inherit a question', () => {
  // The projection sets `question` only while waiting. If one ever rides along
  // on another row, what is shown is this agent's own open question or nothing.
  for (const status of ['idle', 'running', 'done', 'exhausted', 'attached']) {
    const fields = fieldsOf({ ...ROWS[0], status, question: 'Ship 7f3a9c1e or wait for review?' });
    assert.equal(fields.question, '', `a ${status} agent reported a question`);
  }
});

test('an automatic hold reads as held, and only a valid one does', () => {
  const hold = { kind: 'launch_failed', reason: 'no binary' };
  assert.equal(fieldsOf({ ...ROWS[0], status: 'idle', automatic_hold: hold }).status, 'held');
  assert.equal(fieldsOf({ ...ROWS[0], status: 'idle', automatic_hold: { kind: 'other' } }).status, 'idle');
});
