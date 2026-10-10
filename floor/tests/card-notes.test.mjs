// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';
import test from 'node:test';

import { furnish, noteAnchors } from '../src/renderer/office3d/furnish.ts';
import { STUDIO } from '../src/renderer/office3d/studio.ts';
import { cardNotesAt, placeCardNotes, planCardNotes } from '../src/renderer/scene/cardNotes.ts';

const A = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const C = '01ARZ3NDEKTSV4RRFFQ69G5FAX';
const card = (status, claimer = null) => ({ id: C, title: 'Ship board', department: 'engineering',
  status, claimed_by: claimer, status_by: { kind: 'system' }, seq: 0, deps: [] });
const BOARD = { x: 1, y: 2, z: 0 };
const anchors = {
  desks: new Set([A]),
  limit: (place) => ({ board: 6, desk: 3, archive: 6 })[place.kind],
  at: (place, slot) => place.kind === 'board' ? { x: BOARD.x + slot, y: BOARD.y, z: BOARD.z }
    : place.kind === 'archive' ? { x: -5, y: 1 + slot, z: -6 } : { x: 10 + slot, y: 0.85, z: 3 },
};

test('the note placement table follows claims, release, blocked and Done', () => {
  for (const [status, claimer, expected] of [
    ['todo', null, { kind: 'board' }], ['claimed', A, { kind: 'desk', agentId: A }],
    ['doing', A, { kind: 'desk', agentId: A }], ['review', A, { kind: 'desk', agentId: A }],
    ['doing', null, { kind: 'board' }], ['blocked', A, { kind: 'board' }],
    ['done', A, { kind: 'archive' }], ['claimed', 'absent', { kind: 'board' }],
  ]) assert.deepEqual(placeCardNotes([card(status, claimer)], anchors)[0].place, expected);
  assert.deepEqual(placeCardNotes([card('claimed', A)], anchors)[0].at, { x: 10, y: 0.85, z: 3 });
  assert.deepEqual(placeCardNotes([card('done', A)], anchors)[0].at, { x: -5, y: 1, z: -6 });
});

test('notes in one place take its slots in card order', () => {
  const cards = ['1', '2', '3'].map((tail) => ({ ...card('todo'), id: C.slice(0, -1) + tail }));
  assert.deepEqual(placeCardNotes([...cards].reverse(), anchors).map((note) => [note.card.id.at(-1), note.at.x]),
    [['1', 1], ['2', 2], ['3', 3]]);
});

test('accepted deltas move notes from board to desk and archive, including interrupted motion', () => {
  const board = planCardNotes([], placeCardNotes([card('todo')], anchors), 'snapshot', 100);
  const desk = planCardNotes(board, placeCardNotes([card('claimed', A)], anchors), 'delta', 200);
  assert.deepEqual(cardNotesAt(desk, 200, false)[0].at, BOARD);
  assert.deepEqual(cardNotesAt(desk, 400, false)[0].at, { x: 5.5, y: 1.425, z: 1.5 });
  assert.deepEqual(cardNotesAt(desk, 600, false)[0].at, { x: 10, y: 0.85, z: 3 });
  const archive = planCardNotes(desk, placeCardNotes([card('done', A)], anchors), 'delta', 400);
  assert.deepEqual(cardNotesAt(archive, 400, false)[0].at, { x: 5.5, y: 1.425, z: 1.5 });
  assert.deepEqual(cardNotesAt(archive, 800, false)[0].at, { x: -5, y: 1, z: -6 });
});

test('release and blocked deltas return desk notes to the board', () => {
  const desk = planCardNotes([], placeCardNotes([card('doing', A)], anchors), 'snapshot', 100);
  for (const row of [card('doing'), card('blocked', A)]) {
    const board = planCardNotes(desk, placeCardNotes([row], anchors), 'delta', 200);
    assert.deepEqual(cardNotesAt(board, 200, false)[0].at, { x: 10, y: 0.85, z: 3 });
    assert.deepEqual(cardNotesAt(board, 600, false)[0].at, BOARD);
  }
});

test('snapshot replacement removes cards and restart places surviving notes without motion', () => {
  const board = planCardNotes([], placeCardNotes([card('todo')], anchors), 'snapshot', 100);
  const moving = planCardNotes(board, placeCardNotes([card('doing', A)], anchors), 'delta', 200);
  const restart = planCardNotes(moving, placeCardNotes([card('done', A)], anchors), 'snapshot', 300);
  assert.equal(restart[0].startedAt, null);
  assert.deepEqual(cardNotesAt(restart, 300, false)[0].at, { x: -5, y: 1, z: -6 });
  assert.deepEqual(planCardNotes(restart, [], 'snapshot', 400), []);
});

test('reduced motion jumps to the current destination and stable places do not animate', () => {
  const board = planCardNotes([], placeCardNotes([card('todo')], anchors), 'snapshot', 100);
  const desk = planCardNotes(board, placeCardNotes([card('doing', A)], anchors), 'delta', 200);
  assert.deepEqual(cardNotesAt(desk, 200, true)[0].at, { x: 10, y: 0.85, z: 3 });
  const stable = planCardNotes(desk, placeCardNotes([card('review', A)], anchors), 'delta', 700);
  assert.equal(stable[0].startedAt, 200, 'a status change within the same place keeps the note where it is');
  assert.deepEqual(cardNotesAt(stable, 700, false)[0].place, { kind: 'desk', agentId: A });
});

test('the studio hangs card notes on the board, stacks claimed ones on the claimer\'s desk and files done ones', () => {
  const studio = noteAnchors(furnish(STUDIO), new Map([[A, 4]]));
  assert.deepEqual(studio.at({ kind: 'board' }, 0), { x: -9.8, y: 2.25, z: -0.85 });
  assert.deepEqual(studio.at({ kind: 'board' }, 6), studio.at({ kind: 'board' }, 0), 'the seventh board note reuses the first slot');
  const deskNote = studio.at({ kind: 'desk', agentId: A }, 0);
  assert.deepEqual({ x: Number(deskNote.x.toFixed(2)), z: Number(deskNote.z.toFixed(2)) }, { x: -6.3, z: 2.54 });
  assert.deepEqual(studio.at({ kind: 'desk', agentId: A }, 3), studio.at({ kind: 'desk', agentId: A }, 0),
    'a note past the desk\'s last slot travels from its first');
  assert.deepEqual([studio.limit({ kind: 'board' }), studio.limit({ kind: 'desk', agentId: A }), studio.limit({ kind: 'archive' })], [6, 3, 6]);
  assert.deepEqual(studio.at({ kind: 'archive' }, 0), { x: -6.52, y: 1.17, z: -6.55 });
  assert.equal(studio.desks.has(A), true);
});

test('ARCHIVE_PILE mutant: each place draws only as many notes as it has slots, and still counts the rest', () => {
  const many = (count, status, claimer = null) => Array.from({ length: count }, (_, index) =>
    ({ ...card(status, claimer), id: `${C.slice(0, -2)}${String(index).padStart(2, '0')}` }));
  const shown = (notes, kind) => notes.filter((note) => note.place.kind === kind && note.shown).length;
  const placed = placeCardNotes([...many(10, 'done'), ...many(5, 'doing', A).map((row) => ({ ...row, id: `${row.id.slice(0, -3)}D${row.id.slice(-2)}` })),
    ...many(8, 'todo').map((row) => ({ ...row, id: `${row.id.slice(0, -3)}T${row.id.slice(-2)}` }))], anchors);
  assert.deepEqual([shown(placed, 'archive'), shown(placed, 'desk'), shown(placed, 'board')], [6, 3, 6]);
  assert.deepEqual([placed.filter((note) => note.place.kind === 'archive').length, placed.filter((note) => note.place.kind === 'board').length], [10, 8]);
  const later = planCardNotes([], placed, 'snapshot', 100);
  assert.equal(cardNotesAt(later, 100, false).filter((note) => note.shown).length, 15, 'what is drawn follows the slots after planning too');
});
