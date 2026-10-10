// SPDX-License-Identifier: MIT OR Apache-2.0
//
// A layout definition and what is derived from it: the walkable grid from the
// furniture's footprints, the desks from the layout's own list, and the
// approach cell that lets a seat sit inside a footprint.

import assert from 'node:assert/strict';
import test from 'node:test';

import { SEAT_TO_DESK, TURN, deskModel, furnitureModel, turnBox } from '../src/renderer/office3d/catalog.ts';
import { furnish } from '../src/renderer/office3d/furnish.ts';
import { OFFICES } from '../src/renderer/office3d/offices.ts';
import { deskCentre, deskFootprint, footprintAt, footprints, isBlocked, officePlan } from '../src/renderer/office3d/plan.ts';
import { STUDIO } from '../src/renderer/office3d/studio.ts';
import { castFrom } from '../src/renderer/scene/cast.ts';
import { findPath } from '../src/renderer/scene/pathfinding.ts';

function picture(plan) {
  return Array.from({ length: plan.height }, (_, y) => Array.from({ length: plan.width },
    (_, x) => (isBlocked(plan, x, y) ? '#' : '.')).join(''));
}

test('FOOTPRINT_SKIPPED mutant: the studio grid is exactly the union of its furniture, desk and bell footprints', () => {
  assert.deepEqual(picture(officePlan(STUDIO)), [
    '#.##.........#######',
    '....................',
    '#...................',
    '.#...............###',
    '...##.##.##.##..#...',
    '....................',
    '..................##',
    '....................',
    '...............#####',
    '#..##.##.##.##......',
    '................##..',
    '....................',
    '................#...',
    '..............#....#',
  ]);
});

test('a footprint turns with its model, and a desk stands across the two cells in front of its seat', () => {
  assert.deepEqual(footprintAt({ x: 19, y: 8 }, 5, 1, 'west'), { x: 19, y: 8, width: 1, height: 5, centre: { x: 19.5, y: 10.5 } });
  assert.deepEqual(footprintAt({ x: 14, y: 0 }, 5, 1, 'north'), { x: 14, y: 0, width: 5, height: 1, centre: { x: 16.5, y: 0.5 } });
  assert.deepEqual(deskFootprint({ at: { x: 4, y: 3.5 }, facing: 'south' }), { x: 3, y: 4, width: 2, height: 1, centre: { x: 4, y: 4.5 } });
  assert.deepEqual(deskFootprint({ at: { x: 7.5, y: 6 }, facing: 'west' }), { x: 6, y: 5, width: 1, height: 2, centre: { x: 6.5, y: 6 } });
  assert.deepEqual(deskCentre({ at: { x: 4, y: 10.5 }, facing: 'north' }), { x: 4, y: 10.5 - SEAT_TO_DESK });
});

const TINY = {
  ...STUDIO,
  width: 8,
  depth: 6,
  desks: [{ at: { x: 2, y: 1.5 }, facing: 'south' }, { at: { x: 5, y: 1.5 }, facing: 'south' }, { at: { x: 2, y: 4.5 }, facing: 'north' }],
  board: { wall: 'west', along: 3, length: 2, spots: [{ at: { x: 0.5, y: 3.5 }, facing: 'west' }] },
  lounge: [{ at: { x: 7.5, y: 4.5 }, facing: 'west', approach: { x: 6, y: 4 } }],
  bell: { at: { x: 7, y: 0 }, spots: [{ at: { x: 6.5, y: 0.5 }, facing: 'east' }] },
  door: { wall: 'west', along: 5.5, spot: { at: { x: 0.5, y: 5.5 }, facing: 'east' }, queue: [] },
  overflow: { x: 0, y: 0, width: 0, height: 0, facing: 'south' },
  kitchen: { x: 0, y: 0, width: 1, height: 1, facing: 'north' },
  furniture: [{ kind: 'armchair', at: { x: 7, y: 4 }, facing: 'west' }],
  partitions: [],
  decor: [],
  floors: [],
  lamps: [],
};

test('DESK_CONSTANT mutant: the desks, and how many agents sit, come from the layout', () => {
  const plan = officePlan(TINY);
  assert.equal(plan.desks.length, 3);
  const rows = ['01A', '01B', '01C', '01D'].map((id) => ({ id: id.padEnd(26, '0') }));
  const desks = castFrom(rows, plan);
  assert.deepEqual(desks.map((desk) => [desk.foot, desk.seat?.seat ?? null]), [
    [{ x: 2, y: 1.5 }, 0.46], [{ x: 5, y: 1.5 }, 0.46], [{ x: 2, y: 4.5 }, 0.46], [{ x: 3.5, y: 5.5 }, null],
  ]);
  assert.equal(officePlan(STUDIO).desks.length, 8);
});

test('APPROACH_IGNORED mutant: a seat inside a footprint keeps its own cell blocked and is reached from its approach', () => {
  const plan = officePlan(TINY);
  const [seat] = plan.places.lounge;
  assert.equal(isBlocked(plan, 7, 4), true, 'the armchair blocks the seat cell');
  assert.deepEqual(seat, { spot: { x: 7.5, y: 4.5 }, approach: { x: 6, y: 4 }, seat: 0.53, facing: 'west' });
  assert.equal(isBlocked(plan, seat.approach.x, seat.approach.y), false);
  const studio = officePlan(STUDIO);
  for (const lounge of studio.places.lounge) {
    const cell = { x: Math.floor(lounge.spot.x), y: Math.floor(lounge.spot.y) };
    assert.equal(isBlocked(studio, cell.x, cell.y), true, `lounge seat ${JSON.stringify(lounge.spot)} sits inside its sofa or chair`);
    assert.equal(Math.abs(cell.x - lounge.approach.x) + Math.abs(cell.y - lounge.approach.y), 1, 'its approach is next to it');
  }
});

test('every studio station is walkable and reachable from the door', () => {
  const plan = officePlan(STUDIO);
  const walkable = (x, y) => !isBlocked(plan, x, y);
  const stations = [...plan.desks, ...plan.places.board, ...plan.places.lounge, ...plan.places.kitchen, ...plan.places.bell];
  assert.equal(stations.length, 8 + 3 + 4 + 14 + 3);
  for (const station of stations) {
    assert.ok(walkable(station.approach.x, station.approach.y), `approach ${JSON.stringify(station.approach)} is walkable`);
    assert.ok(findPath(plan.door.approach, station.approach, walkable, plan) !== null, `${JSON.stringify(station.spot)} is reachable`);
  }
});

test('MODEL_OVERHANG mutant: every model stays inside the footprint it blocks', () => {
  const kinds = new Set(OFFICES.flatMap((office) => office.definition.furniture.map((item) => item.kind)));
  for (const [kind, model] of [...kinds].flatMap((kind) => [0, 1, 2, 3].map((variant) => [kind, furnitureModel(kind, variant)]))) {
    for (const facing of ['south', 'east', 'north', 'west']) {
      const print = footprintAt({ x: 0, y: 0 }, model.width, model.depth, facing);
      for (const box of model.boxes.map((b) => turnBox(b, TURN[facing], { x: print.width / 2, y: 0, z: print.height / 2 }))) {
        assert.ok(box.at.x - box.size.x / 2 >= -0.001 && box.at.x + box.size.x / 2 <= print.width + 0.001
          && box.at.z - box.size.z / 2 >= -0.001 && box.at.z + box.size.z / 2 <= print.height + 0.001,
        `${kind} facing ${facing} reaches outside its ${print.width}x${print.height} footprint`);
      }
    }
  }
  for (const { definition } of OFFICES) {
    for (const box of [0, 1].flatMap((variant) => deskModel(variant, definition.style.desk).boxes)) {
      assert.ok(Math.abs(box.at.x) + box.size.x / 2 <= 1 && box.at.z + box.size.z / 2 <= 1 - SEAT_TO_DESK + 0.5,
        `a ${definition.label} desk reaches no further than the two cells in front of its seat`);
    }
  }
});

const key = (cell) => `${cell.x},${cell.y}`;

/** A minimal layout for `boardRoom` doorway cases: a board at (1.5, 1.5) in an otherwise empty 6x6 or 7x6 grid. */
const DOORWAY_BASE = {
  ...STUDIO,
  desks: [],
  lounge: [],
  bell: { at: { x: 5, y: 5 }, spots: [] },
  overflow: { x: 0, y: 0, width: 0, height: 0, facing: 'south' },
  kitchen: { x: 0, y: 0, width: 0, height: 0, facing: 'north' },
  furniture: [],
  decor: [],
  floors: [],
  lamps: [],
  board: { wall: 'west', along: 1.5, length: 3, spots: [{ at: { x: 1.5, y: 1.5 }, facing: 'west' }] },
};

/** A 3x3 room (x0-2, y0-2) whose south wall stops one cell short of the grid's west edge: the gap's west
 * jamb is the floor's own edge, not a partition cell. */
const WEST_EDGE_DOORWAY = {
  ...DOORWAY_BASE,
  width: 6,
  depth: 6,
  door: { wall: 'north', along: 4.5, spot: { at: { x: 4.5, y: 0.5 }, facing: 'south' }, queue: [] },
  partitions: [
    { from: { x: 3, y: 0 }, to: { x: 3, y: 2 } },
    { from: { x: 1, y: 3 }, to: { x: 3, y: 3 } },
  ],
};

/** The same room, turned: its east wall stops one cell short of the grid's north edge. */
const NORTH_EDGE_DOORWAY = {
  ...DOORWAY_BASE,
  width: 6,
  depth: 6,
  door: { wall: 'west', along: 4.5, spot: { at: { x: 0.5, y: 4.5 }, facing: 'east' }, queue: [] },
  partitions: [
    { from: { x: 0, y: 3 }, to: { x: 3, y: 3 } },
    { from: { x: 3, y: 1 }, to: { x: 3, y: 2 } },
  ],
};

/** A room whose south wall leaves a two-cell gap, flanked by partition cells on both sides. */
const TWO_CELL_DOORWAY = {
  ...DOORWAY_BASE,
  width: 7,
  depth: 6,
  door: { wall: 'north', along: 5.5, spot: { at: { x: 5.5, y: 0.5 }, facing: 'south' }, queue: [] },
  partitions: [
    { from: { x: 4, y: 0 }, to: { x: 4, y: 2 } },
    { from: { x: 0, y: 3 }, to: { x: 0, y: 3 } },
    { from: { x: 3, y: 3 }, to: { x: 4, y: 3 } },
  ],
};

/** The Studio variant: a one-cell-wide board room with no doorway at all into the main
 * room, so the queue cell it swallows proves the walk leaked rather than stopped at a wall. */
const SWALLOWED_OFFICE = {
  ...DOORWAY_BASE,
  width: 6,
  depth: 5,
  door: { wall: 'west', along: 4.5, spot: { at: { x: 1.5, y: 4.5 }, facing: 'north' }, queue: [{ at: { x: 4.5, y: 1.5 }, facing: 'west' }] },
  partitions: [{ from: { x: 0, y: 3 }, to: { x: 5, y: 3 } }],
};

/** The west-edge room again, with a lounge chair placed inside it: a real walled room, not a leak, so its
 * boardRoom stays exactly what it was without the chair. */
const LOUNGE_IN_ROOM = {
  ...WEST_EDGE_DOORWAY,
  lounge: [{ at: { x: 2.5, y: 2.5 }, facing: 'north' }],
};

test('DOORWAY_JAMB_PARTITION_ONLY mutant: a doorway against the grid\'s own edge still stops the board\'s room, on the west wall and the north wall', () => {
  assert.deepEqual(officePlan(WEST_EDGE_DOORWAY).boardRoom.map(key).sort(),
    ['0,0', '0,1', '0,2', '0,3', '1,0', '1,1', '1,2', '2,0', '2,1', '2,2'], 'the 3x3 room plus its doorway against the west edge');
  assert.deepEqual(officePlan(NORTH_EDGE_DOORWAY).boardRoom.map(key).sort(),
    ['0,0', '0,1', '0,2', '1,0', '1,1', '1,2', '2,0', '2,1', '2,2', '3,0'], 'the 3x3 room plus its doorway against the north edge');
});

test('DOORWAY_WIDE_GAP_MISSED mutant: a two-cell doorway still stops the board\'s room', () => {
  assert.deepEqual(officePlan(TWO_CELL_DOORWAY).boardRoom.map(key).sort(),
    ['0,0', '0,1', '0,2', '1,0', '1,1', '1,2', '1,3', '2,0', '2,1', '2,2', '2,3', '3,0', '3,1', '3,2'],
    'the room plus both cells of its doorway');
});

test('ROOM_OCCUPIED_UNCHECKED mutant: a board room that reaches the door\'s queue without crossing a doorway is rejected, not returned whole', () => {
  const plan = officePlan(SWALLOWED_OFFICE);
  assert.equal(plan.boardRoom.length, 0, 'the walk leaked past the open half of the floor instead of stopping at a wall');
  assert.equal(plan.door.approach.x === 1 && plan.door.approach.y === 4, true, 'sanity: the door sits on the other, unreached side of the floor');
});

test('ROOM_LOUNGE_FALSE_REJECT mutant: a real walled room keeps its boardRoom when it legitimately holds a lounge chair', () => {
  const withoutChair = officePlan(WEST_EDGE_DOORWAY).boardRoom.map(key).sort();
  const withChair = officePlan(LOUNGE_IN_ROOM).boardRoom.map(key).sort();
  assert.deepEqual(withChair, withoutChair, 'the lounge chair sits in the room\'s own open floor, so the room is unchanged by it');
  assert.ok(officePlan(LOUNGE_IN_ROOM).places.lounge.some((seat) => withChair.includes(key(seat.approach))),
    'sanity: the chair really does sit inside the room this asserts stays whole');
});

test('the studio furnishes every desk, the bell, the board notes and the shipped shelf', () => {
  const furnished = furnish(STUDIO);
  const count = (kind) => furnished.fixtures.filter((fixture) => fixture.kind === kind).length;
  assert.equal(count('screen-line'), 8 * 3);
  assert.equal(new Set(furnished.fixtures.filter((f) => f.kind === 'screen-line').map((f) => f.desk)).size, 8);
  assert.equal(count('bell'), 1);
  assert.equal(count('shelf-slot'), 10);
  assert.equal(count('goal-note'), 8);
  assert.equal(count('card-note'), 6);
  assert.equal(count('archive-slot'), 6);
  assert.equal(footprints(STUDIO).length, STUDIO.furniture.length + STUDIO.desks.length + 1);
  const bell = furnished.fixtures.find((fixture) => fixture.kind === 'bell');
  assert.deepEqual(bell.at, { x: 6.5, y: 1, z: -2.5 });
});
