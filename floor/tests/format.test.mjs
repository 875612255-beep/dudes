// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';
import test from 'node:test';

import { journalPosition, recordedTime } from '../src/renderer/format.ts';

test('journal positions show the segment and byte offset without losing u64 precision', () => {
  assert.equal(journalPosition('2199023259734'), 'segment 2 · byte 4182');
  assert.equal(journalPosition('18446744073709551615'), 'segment 16777215 · byte 1099511627775');
  assert.equal(journalPosition(0), 'journal has no records yet');
  assert.equal(journalPosition(Number.MAX_SAFE_INTEGER + 1), 'Position unavailable');
});

test('recorded time uses the local clock and names its timezone', () => {
  const previous = process.env.TZ;
  try {
    process.env.TZ = 'Pacific/Auckland';
    assert.equal(recordedTime('2026-09-28T12:34:10.668Z'), 'Sep 29, 2026, 1:34:10 AM GMT+13');
    process.env.TZ = 'America/Los_Angeles';
    assert.equal(recordedTime('2026-09-28T12:34:10.668Z'), 'Sep 28, 2026, 5:34:10 AM PDT');
    assert.equal(recordedTime(NaN), 'Recorded time unavailable');
  } finally {
    if (previous === undefined) delete process.env.TZ;
    else process.env.TZ = previous;
  }
});
