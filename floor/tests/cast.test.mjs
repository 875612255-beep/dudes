// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Firing controls for this file:
//
//  1. seating — take the layout's desks in reverse: "the first agents take the
//     desk seats, in the layout's order" goes red naming the seat that moved.
//  2. overflow — return no cells from `standingCells`: "an agent past the last
//     seat queues by the door" goes red, two characters short; drop its check
//     that the door reaches a cell and the same test goes red on the cell
//     walled in at 1,0.
//  3. no roster ships — paste any 26-character ULID into a file under
//     `floor/src/`: "no agent id is written into the shipped floor" goes red
//     naming the file and the literal.

import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

import { isBlocked, officePlan } from '../src/renderer/office3d/plan.ts';
import { HQ } from '../src/renderer/office3d/hq.ts';
import { STUDIO } from '../src/renderer/office3d/studio.ts';
import { castFrom } from '../src/renderer/scene/cast.ts';

const floorRoot = resolve(import.meta.dirname, '..');
const plan = officePlan(STUDIO);
const ADA = '01JBQ8XK2M3N4P5Q6R7S8T9VWX';
const GRACE = '01JBQ8XK2M3N4P5Q6R7S8T9VWY';
const SUFFIXES = 'ABCDEFGHJKMNPQRSTVWXYZ'.split('').flatMap((a) => '23456789'.split('').map((b) => `${a}${b}`));
const rows = (count) => SUFFIXES.slice(0, count).map((suffix) => ({ id: `${ADA.slice(0, -2)}${suffix}` }));

test('the first agents take the desk seats, in the layout\'s order', () => {
  const desks = castFrom(rows(8), plan);
  assert.deepEqual(desks.map((desk) => desk.foot), [
    { x: 4, y: 3.5 }, { x: 7, y: 3.5 }, { x: 10, y: 3.5 }, { x: 13, y: 3.5 },
    { x: 4, y: 10.5 }, { x: 7, y: 10.5 }, { x: 10, y: 10.5 }, { x: 13, y: 10.5 },
  ]);
  assert.deepEqual(desks.map((desk) => desk.seat?.facing), ['south', 'south', 'south', 'south', 'north', 'north', 'north', 'north']);
});

test('two agents become two desks, one per seat, in the order given', () => {
  const desks = castFrom([{ id: GRACE }, { id: ADA }], plan);
  assert.deepEqual(desks.map((desk) => [desk.agentId, desk.foot]), [[GRACE, { x: 4, y: 3.5 }], [ADA, { x: 7, y: 3.5 }]]);
});

test('OVERFLOW_SKIPPED mutant: an agent past the last seat queues by the door, then stands in the overflow area, and past both nearest the overflow area', () => {
  const desks = castFrom(rows(24), plan);
  assert.equal(desks.length, 24);
  assert.deepEqual(desks.slice(8).map((desk) => [desk.foot, desk.seat?.facing ?? null, desk.seat?.seat ?? null]), [
    ...[1.5, 2.5, 3.5, 4.5, 5.5, 6.5].map((x) => [{ x, y: 13.5 }, 'west', null]),
    ...[4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5, 11.5].map((x) => [{ x, y: 1.5 }, 'south', null]),
    [{ x: 4.5, y: 0.5 }, null, null], [{ x: 4.5, y: 2.5 }, null, null],
  ]);
  for (const desk of desks) assert.equal(isBlocked(plan, Math.floor(desk.foot.x), Math.floor(desk.foot.y)), false);
  const cells = desks.map((desk) => `${Math.floor(desk.foot.x)},${Math.floor(desk.foot.y)}`);
  assert.equal(new Set(cells).size, 24, 'no two agents share a cell');
  const walledIn = officePlan({ ...HQ, desks: [], door: { ...HQ.door, queue: [] }, overflow: { ...HQ.overflow, width: 0 },
    furniture: [...HQ.furniture, ...[[0, 0], [2, 0], [1, 1]].map(([x, y]) => ({ kind: 'plant', at: { x, y } }))] });
  const crowd = castFrom(Array.from({ length: 500 }, (_, k) => ({ id: `${ADA.slice(0, -3)}${String(k).padStart(3, '0')}` })), walledIn);
  let free = 0;
  for (let y = 0; y < walledIn.height; y += 1) for (let x = 0; x < walledIn.width; x += 1) if (!isBlocked(walledIn, x, y)) free += 1;
  assert.equal(crowd.length, free - 8, 'every free cell takes an agent but the seven in the server room and the one walled in at 1,0');
  assert.equal(crowd.some((desk) => Math.floor(desk.foot.x) === 1 && Math.floor(desk.foot.y) === 0), false, 'the cell walled in at 1,0 is passed over');
});

// The one way a fixture-only AgentId could reach the screen is by being written
// into the floor's own source, so nothing else has to be trusted: this fails if
// one is. The pattern is the contract's own ULID text form.
const ULID_RUN = /(?<![0-9A-Z])[0-7][0-9A-HJKMNP-TV-Z]{25}(?![0-9A-Z])/;

async function sourceFiles(dir) {
  const found = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) found.push(...await sourceFiles(path));
    else if (entry.isFile() && /\.(ts|tsx|css|html|json)$/.test(entry.name)) found.push(path);
  }
  return found;
}

test('no agent id is written into the shipped floor', async () => {
  // Source and build output both: `npm test` builds first, so this reads the
  // bytes the daemon would actually serve, not only what was written by hand.
  const built = (await readdir(resolve(floorRoot, 'dist/assets')))
    .filter((name) => name.endsWith('.js') || name.endsWith('.css'))
    .map((name) => resolve(floorRoot, 'dist/assets', name));
  const files = [
    ...await sourceFiles(resolve(floorRoot, 'src')),
    resolve(floorRoot, 'index.html'),
    resolve(floorRoot, 'dist/index.html'),
    ...built,
  ];
  assert.ok(built.length > 0, 'the scan reached the build output');
  assert.ok(files.length > 5, 'the scan found files to scan');
  for (const path of files) {
    const text = await readFile(path, 'utf8');
    const hit = ULID_RUN.exec(text);
    assert.equal(hit, null, `${path} carries the agent id ${hit?.[0] ?? ''}`);
  }
  assert.notEqual(ULID_RUN.exec(`const roster = ['${ADA}'];`), null);
});
