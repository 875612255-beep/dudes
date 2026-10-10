// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Firing control for this file: make `findPath` ignore its `walkable` argument
// and treat every in-bounds cell as passable — "the route goes around a wall,
// not through it" and "a sealed goal has no route" both go red.

import assert from 'node:assert/strict';
import test from 'node:test';

import { NEIGHBOURS, findPath } from '../src/renderer/scene/pathfinding.ts';

const OPEN_5x5 = { width: 5, height: 5 };
const open = () => true;

/**
 * A 5 x 5 grid with a vertical wall at x = 2, open only at y = 4:
 *
 *   . . # . .
 *   . . # . .
 *   . . # . .
 *   . . # . .
 *   . . . . .
 */
function walledGrid(x, y) {
  return !(x === 2 && y !== 4);
}

test('an open grid gives the straight line, cell for cell', () => {
  assert.deepEqual(
    findPath({ x: 0, y: 0 }, { x: 3, y: 0 }, open, OPEN_5x5),
    [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 2, y: 0 }, { x: 3, y: 0 }],
  );
  assert.deepEqual(
    findPath({ x: 1, y: 3 }, { x: 1, y: 1 }, open, OPEN_5x5),
    [{ x: 1, y: 3 }, { x: 1, y: 2 }, { x: 1, y: 1 }],
  );
});

test('the route goes around a wall, not through it', () => {
  const path = findPath({ x: 1, y: 0 }, { x: 3, y: 0 }, walledGrid, OPEN_5x5);
  assert.ok(path !== null, 'a route around the wall exists and must be found');

  // Every cell the search hands back must satisfy the callback it was given.
  // This is the assertion an always-true walkability mutant breaks.
  for (const [at, cell] of path.entries()) {
    assert.ok(
      walledGrid(cell.x, cell.y),
      `step ${at} of the route stands on blocked cell ${cell.x},${cell.y}`,
    );
  }

  // Down the near side, through the gap at y = 4, and back up: 11 cells, not
  // the 3 of a straight line through the wall.
  assert.equal(path.length, 11, 'the detour is 11 cells long');
  assert.deepEqual(path, [
    { x: 1, y: 0 },
    { x: 1, y: 1 },
    { x: 1, y: 2 },
    { x: 1, y: 3 },
    { x: 1, y: 4 },
    { x: 2, y: 4 },
    { x: 3, y: 4 },
    { x: 3, y: 3 },
    { x: 3, y: 2 },
    { x: 3, y: 1 },
    { x: 3, y: 0 },
  ]);
});

test('every step of a route is a single four-neighbour move', () => {
  const path = findPath({ x: 0, y: 0 }, { x: 4, y: 4 }, walledGrid, OPEN_5x5);
  assert.ok(path !== null);
  for (let at = 1; at < path.length; at += 1) {
    const dx = path[at].x - path[at - 1].x;
    const dy = path[at].y - path[at - 1].y;
    assert.ok(
      NEIGHBOURS.some((step) => step.x === dx && step.y === dy),
      `step ${at} moved by ${dx},${dy}, which is not a four-neighbour move`,
    );
  }
});

test('a sealed goal has no route', () => {
  // The same wall, with the y = 4 gap closed: nothing reaches the right side.
  const sealed = (x) => x !== 2;
  assert.equal(findPath({ x: 0, y: 0 }, { x: 4, y: 4 }, sealed, OPEN_5x5), null);
  assert.equal(findPath({ x: 4, y: 0 }, { x: 0, y: 0 }, sealed, OPEN_5x5), null);
  // And the reachable side still works, so the grid is not simply impassable.
  assert.deepEqual(
    findPath({ x: 0, y: 0 }, { x: 1, y: 0 }, sealed, OPEN_5x5),
    [{ x: 0, y: 0 }, { x: 1, y: 0 }],
  );
});

test('standing on the goal is a one-cell route', () => {
  assert.deepEqual(findPath({ x: 2, y: 2 }, { x: 2, y: 2 }, open, OPEN_5x5), [{ x: 2, y: 2 }]);
});

test('an endpoint off the grid or on a blocked cell has no route', () => {
  assert.equal(findPath({ x: -1, y: 0 }, { x: 2, y: 2 }, open, OPEN_5x5), null);
  assert.equal(findPath({ x: 0, y: 0 }, { x: 5, y: 0 }, open, OPEN_5x5), null);
  assert.equal(findPath({ x: 0, y: 0 }, { x: 0, y: 99 }, open, OPEN_5x5), null);
  assert.equal(findPath({ x: 0.5, y: 0 }, { x: 2, y: 2 }, open, OPEN_5x5), null);
  // Blocked start, then blocked goal.
  assert.equal(findPath({ x: 2, y: 0 }, { x: 0, y: 0 }, walledGrid, OPEN_5x5), null);
  assert.equal(findPath({ x: 0, y: 0 }, { x: 2, y: 0 }, walledGrid, OPEN_5x5), null);
});

test('the route is deterministic, so a character does not twitch between frames', () => {
  const first = findPath({ x: 0, y: 0 }, { x: 4, y: 4 }, open, OPEN_5x5);
  for (let run = 0; run < 5; run += 1) {
    assert.deepEqual(findPath({ x: 0, y: 0 }, { x: 4, y: 4 }, open, OPEN_5x5), first);
  }
  // Fixed neighbour order up, right, down, left means the right-hand cell of a
  // layer is always dequeued first, so an open diagonal run takes every step
  // right before it takes any step down. That exact route is the contract.
  assert.deepEqual(first, [
    { x: 0, y: 0 },
    { x: 1, y: 0 },
    { x: 2, y: 0 },
    { x: 3, y: 0 },
    { x: 4, y: 0 },
    { x: 4, y: 1 },
    { x: 4, y: 2 },
    { x: 4, y: 3 },
    { x: 4, y: 4 },
  ]);
});

test('the search is shortest-first, not merely connected', () => {
  // Manhattan distance is the lower bound on an open grid, and BFS attains it.
  for (const goal of [{ x: 4, y: 0 }, { x: 0, y: 4 }, { x: 4, y: 4 }, { x: 3, y: 2 }]) {
    const path = findPath({ x: 0, y: 0 }, goal, open, OPEN_5x5);
    assert.equal(path.length, goal.x + goal.y + 1, `route to ${goal.x},${goal.y} is not shortest`);
  }
});

test('a walkability callback is asked only about cells on the grid', () => {
  const asked = [];
  findPath({ x: 0, y: 0 }, { x: 4, y: 4 }, (x, y) => {
    asked.push({ x, y });
    return true;
  }, OPEN_5x5);
  for (const cell of asked) {
    assert.ok(
      cell.x >= 0 && cell.y >= 0 && cell.x < 5 && cell.y < 5,
      `the search asked about off-grid cell ${cell.x},${cell.y}`,
    );
  }
});

test('a degenerate grid size is refused', () => {
  assert.throws(() => findPath({ x: 0, y: 0 }, { x: 0, y: 0 }, open, { width: 0, height: 5 }), /grid size/);
  assert.throws(() => findPath({ x: 0, y: 0 }, { x: 0, y: 0 }, open, { width: 5, height: -1 }), /grid size/);
});
