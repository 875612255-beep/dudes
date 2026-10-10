// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Every office the Floor can draw, held to the same rules, and the choice of
// office for a crew.
//
// Mutation controls, each named by the test it turns red:
//   - SMALLER_ROUNDS_UP: `entry.capacity >= crew` becomes `>` in chooseOffice ... "the smallest office that seats..."
//   - PIN_IGNORED: chooseOffice drops the pinned lookup ......................... "a pin wins in both directions"
//   - PIN_ONLY_UPWARD: a pin counts only when it seats the crew ................. "a pin wins in both directions"
//   - FULL_NEVER: `full` is always false ........................................ "the smallest office that seats..."
//   - UNKNOWN_PIN_KEPT: readOfficePin returns whatever is stored ................ "an unknown stored pin reads as Automatic"
//   - PARTITION_UNBLOCKED: partitions leave footprints() ........................ "nothing drawn on the floor..."
//   - DESK_DROPPED: HQ loses its last pod ....................................... "every office seats its capacity..."
//   - QUEUE_SEALS_DOOR: the HQ queue starts beside the door instead of below it . "a full queue leaves..."
//   - BOARD_COLUMNS_FIXED: boardModel draws four goal columns whatever its length "BOARD_COLUMNS_FIXED mutant..."
//   - FALLBACK_FROM_DOOR: standingCells spreads out from the door, not the overflow area ... "FALLBACK_FROM_DOOR, ..."
//   - FALLBACK_SEALS: standingCells keeps a cell that walls off a seat or place .......... "FALLBACK_FROM_DOOR, ..."
//   - FALLBACK_BOXES: standingCells keeps a cell that boxes someone in, itself included .. "FALLBACK_FROM_DOOR, ..."
//   - FALLBACK_NEAR_BOARD: standingCells keeps a cell by the board, the door or the queue "FALLBACK_FROM_DOOR, ..."
//   - DESKS_PASSABLE: standingCells walks through the agents at desks ..................... "FALLBACK_FROM_DOOR, ..."
//   - BOARD_ROOM_OPEN: the plan finds no room around any board ........................... "BOARD_ROOM_OPEN mutant: ..."
//   - BOARD_ROOM_SKIPPED: standingCells lets agents stand in the board's room ............. "FALLBACK_FROM_DOOR, ..."

import assert from 'node:assert/strict';
import test from 'node:test';

import { boardModel, furnitureModel } from '../src/renderer/office3d/catalog.ts';
import { furnish } from '../src/renderer/office3d/furnish.ts';
import { OFFICES, chooseOffice, followFull, fullNotice } from '../src/renderer/office3d/offices.ts';
import { footprintAt, isBlocked, officePlan } from '../src/renderer/office3d/plan.ts';
import { DEFAULT_AZIMUTH } from '../src/renderer/office3d/view.ts';
import { castFrom, standingCells } from '../src/renderer/scene/cast.ts';
import { findPath } from '../src/renderer/scene/pathfinding.ts';
import { readOfficePin, writeOfficePin } from '../src/renderer/view.ts';

const key = (cell) => `${cell.x},${cell.y}`;

function stationsOf(plan) {
  return [...plan.desks, ...plan.places.board, ...plan.places.lounge, ...plan.places.kitchen, ...plan.places.bell, ...plan.queue, ...plan.overflow];
}

function reachableFromDoor(plan, closed = new Set()) {
  const walkable = (x, y) => !isBlocked(plan, x, y) && !closed.has(`${x},${y}`);
  return (cell) => findPath(plan.door.approach, cell, walkable, plan) !== null;
}

test('every office seats its capacity, and every seat and place it sends an agent to is walkable and reached from the door', () => {
  for (const { id, capacity, definition } of OFFICES) {
    const plan = officePlan(definition);
    assert.equal(plan.desks.length, capacity, `${id} has a desk for each agent it seats`);
    const reached = reachableFromDoor(plan);
    for (const station of stationsOf(plan)) {
      const where = `${id} ${JSON.stringify(station.spot)}`;
      assert.equal(isBlocked(plan, station.approach.x, station.approach.y), false, `${where} is walked to over a free cell`);
      assert.ok(reached(station.approach), `${where} is reached from the door`);
      const cell = { x: Math.floor(station.spot.x), y: Math.floor(station.spot.y) };
      if (key(cell) !== key(station.approach)) {
        assert.equal(Math.abs(cell.x - station.approach.x) + Math.abs(cell.y - station.approach.y), 1,
          `${where} sits in a footprint next to the cell it is reached from`);
      }
    }
  }
});

test('the only free cells the door does not reach are inside the HQ server room', () => {
  const unreached = Object.fromEntries(OFFICES.map(({ id, definition }) => {
    const plan = officePlan(definition);
    const reached = reachableFromDoor(plan);
    const cells = [];
    for (let y = 0; y < plan.height; y += 1) {
      for (let x = 0; x < plan.width; x += 1) if (!isBlocked(plan, x, y) && !reached({ x, y })) cells.push(key({ x, y }));
    }
    return [id, cells];
  }));
  assert.deepEqual(unreached, { studio: [], workshop: [], hq: ['0,15', '0,16', '1,16', '2,16', '0,17', '1,17', '2,17'] });
});

test('OVERFLOW_SEALS_BOARD mutant: a full queue by the door and a full overflow area leave every seat and place reachable', () => {
  for (const { id, definition } of OFFICES) {
    const plan = officePlan(definition);
    assert.ok(plan.queue.length >= 4, `${id} queues at least four agents`);
    const standing = new Set([...plan.queue, ...plan.overflow].map((spot) => key(spot.approach)));
    const reached = reachableFromDoor(plan, standing);
    for (const station of stationsOf(plan).filter((spot) => !standing.has(key(spot.approach)))) {
      assert.ok(reached(station.approach), `${id} ${JSON.stringify(station.spot)} is reached past the queue and the overflow area`);
    }
  }
});

test('OVERFLOW_IN_CHECK_ROOM mutant: agents past the door queue stand in each office\'s overflow area, away from the goal board and the door', () => {
  const placed = Object.fromEntries(OFFICES.map(({ id, capacity, definition }) => {
    const plan = officePlan(definition);
    const crew = capacity + plan.queue.length + plan.overflow.length;
    const rows = Array.from({ length: crew }, (_, k) => ({ id: `01JBQ8XK2M3N4P5Q6R7S8T9V${String(k).padStart(2, '0')}` }));
    const overflow = castFrom(rows, plan).slice(capacity + plan.queue.length);
    const near = (cell, spots) => spots.some((spot) => Math.max(Math.abs(spot.approach.x - cell.x), Math.abs(spot.approach.y - cell.y)) <= 2);
    for (const desk of overflow) {
      const cell = { x: Math.floor(desk.foot.x), y: Math.floor(desk.foot.y) };
      assert.equal(near(cell, [...plan.places.board, plan.door, ...plan.queue]), false, `${id} ${key(cell)} stands clear of the board, the door and its queue`);
    }
    return [id, overflow.map((desk) => key({ x: Math.floor(desk.foot.x), y: Math.floor(desk.foot.y) }))];
  }));
  const row = (y, from, to) => Array.from({ length: to - from + 1 }, (_, k) => `${from + k},${y}`);
  assert.deepEqual(placed, { studio: row(1, 4, 11), workshop: row(1, 9, 16), hq: row(1, 7, 16) });
});

test('BOARD_ROOM_OPEN mutant: HQ\'s board hangs in the glass check room, and the other boards in the room the door opens into', () => {
  const rooms = Object.fromEntries(OFFICES.map(({ id, definition }) => [id, officePlan(definition).boardRoom.map(key).sort()]));
  const checkRoom = [];
  for (let y = 0; y <= 4; y += 1) for (let x = 0; x <= 4; x += 1) if (!(y === 3 && x >= 1 && x <= 3)) checkRoom.push(key({ x, y }));
  assert.deepEqual(rooms, { studio: [], workshop: [], hq: [...checkRoom, '2,5'].sort() }, 'the check room, around its meeting table, and its doorway');
});

/** The walkable cells a walk from the door reaches without crossing a cell in `closed`. */
function floodFromDoor(plan, closed) {
  const reached = new Set([key(plan.door.approach)]);
  const queue = [plan.door.approach];
  for (let at = 0; at < queue.length; at += 1) {
    for (const [dx, dy] of [[0, -1], [1, 0], [0, 1], [-1, 0]]) {
      const next = { x: queue[at].x + dx, y: queue[at].y + dy };
      if (isBlocked(plan, next.x, next.y) || closed.has(key(next)) || reached.has(key(next))) continue;
      reached.add(key(next));
      queue.push(next);
    }
  }
  return reached;
}

test('FALLBACK_FROM_DOOR, FALLBACK_SEALS, FALLBACK_BOXES, FALLBACK_NEAR_BOARD, DESKS_PASSABLE and BOARD_ROOM_SKIPPED mutants: every crew up to each office\'s clear cells gets a cell of its own, clear of the board\'s room, the door and its queue, and no one walls off a place or boxes anyone in', () => {
  const cellOf = (point) => ({ x: Math.floor(point.x), y: Math.floor(point.y) });
  const near = (cell, spots) => spots.some((spot) => Math.max(Math.abs(spot.approach.x - cell.x), Math.abs(spot.approach.y - cell.y)) <= 2);
  const sizes = {};
  const at40 = {};
  for (const { id, capacity, definition } of OFFICES) {
    const plan = officePlan(definition);
    const designed = capacity + plan.queue.length + plan.overflow.length;
    const largest = designed + standingCells(plan).clear.length;
    sizes[id] = largest;
    const crew = Array.from({ length: largest }, (_, k) => ({ id: `01JBQ8XK2M3N4P5Q6R7S8T${String(k).padStart(4, '0')}` }));
    const places = new Set(stationsOf(plan).flatMap((station) => [key(station.approach), key(cellOf(station.spot))]));
    const room = new Set(plan.boardRoom.map(key));
    // An agent blocks its cell for the others' walks, a seated one included, so each needs a reached cell beside it to walk out to.
    const boxedIn = (held) => {
      const reached = floodFromDoor(plan, held);
      return [...held].filter((cell) => {
        const [x, y] = cell.split(',').map(Number);
        return ![[0, -1], [1, 0], [0, 1], [-1, 0]].some(([dx, dy]) => reached.has(key({ x: x + dx, y: y + dy })));
      });
    };
    const boxedByDesign = boxedIn(new Set(castFrom(crew.slice(0, designed), plan).map((desk) => key(cellOf(desk.foot)))));
    for (let count = 1; count <= largest; count += 1) {
      const desks = castFrom(crew.slice(0, count), plan);
      const where = `${id} with ${count}`;
      assert.equal(desks.length, count, `${where}: every agent is drawn`);
      const held = new Set(desks.map((desk) => key(cellOf(desk.foot))));
      assert.equal(held.size, count, `${where}: no two agents share a cell`);
      const past = desks.slice(designed).map((desk) => cellOf(desk.foot));
      for (const cell of past) {
        assert.equal(near(cell, [...plan.places.board, plan.door, ...plan.queue]), false, `${where}: ${key(cell)} stands clear of the board, the door and its queue`);
        assert.equal(room.has(key(cell)) || (id === 'hq' && cell.x <= 4 && cell.y <= 4), false, `${where}: ${key(cell)} is outside the room the board hangs in`);
        assert.equal(places.has(key(cell)), false, `${where}: no place sends an agent to ${key(cell)}`);
      }
      const reached = floodFromDoor(plan, held);
      for (const station of stationsOf(plan).filter((spot) => !held.has(key(spot.approach)))) {
        assert.ok(reached.has(key(station.approach)), `${where}: ${JSON.stringify(station.spot)} is still reached from the door`);
      }
      assert.deepEqual(boxedIn(held).filter((cell) => !boxedByDesign.includes(cell)), [], `${where}: everyone, at a desk or standing, can still walk out`);
      if (count === 40) at40[id] = past.map(key);
    }
  }
  assert.deepEqual(sizes, { studio: 85, workshop: 153, hq: 174 }, 'the crews each office holds this way');
  // Nearest the overflow area first, so HQ's eight past it stay by its overflow row, far from the glass check room.
  assert.deepEqual(at40.hq, ['7,2', '17,1', '7,3', '8,3', '9,3', '10,3', '11,3', '12,3']);
});

/**
 * The cells a box stands over. A figure fills the middle of the cell it walks
 * through, so a box may reach a quarter of a cell into a walkable one, as a
 * desk's edge does over its chair and a window sill does over the floor.
 */
function cellsUnder(box, layout) {
  const reach = 0.25;
  const span = (centre, size, offset) => [Math.floor(centre - size / 2 + offset + reach), Math.ceil(centre + size / 2 + offset - reach) - 1];
  const [x0, x1] = span(box.at.x, box.size.x, layout.width / 2);
  const [y0, y1] = span(box.at.z, box.size.z, layout.depth / 2);
  const cells = [];
  for (let y = Math.max(0, y0); y <= Math.min(layout.depth - 1, y1); y += 1) {
    for (let x = Math.max(0, x0); x <= Math.min(layout.width - 1, x1); x += 1) cells.push({ x, y });
  }
  return cells;
}

test('nothing drawn on the floor stands over a cell an agent walks through, but the chair at each desk', () => {
  for (const { id, definition } of OFFICES) {
    const plan = officePlan(definition);
    const furnished = furnish(definition);
    const seats = definition.desks.map((seat) => ({ x: seat.at.x - definition.width / 2, z: seat.at.y - definition.depth / 2 }));
    const chair = (box) => seats.some((seat) => Math.abs(box.at.x - seat.x) < 0.3 && Math.abs(box.at.z - seat.z) < 0.3);
    const standing = [...furnished.boxes, ...furnished.fixtures.flatMap((fixture) => (fixture.box === undefined ? [] : [fixture.box]))]
      .filter((box) => box.at.y + box.size.y / 2 > 0.1 && box.at.y - box.size.y / 2 < 1.7 && !chair(box));
    const crossings = standing.flatMap((box) => cellsUnder(box, definition)
      .filter((cell) => !isBlocked(plan, cell.x, cell.y)).map((cell) => `${box.colour} ${JSON.stringify(box.size)} over ${key(cell)}`));
    assert.deepEqual(crossings, [], `${id} draws nothing over a walkable cell`);
    assert.ok(standing.length > 100, `${id} has furniture to check`);
  }
});

test('BOARD_COLUMNS_FIXED mutant: each board fits its goal notes before its card notes, so no two notes are drawn in one place', () => {
  const columns = Object.fromEntries(OFFICES.map(({ id, definition }) => {
    const slots = boardModel(definition.board.length, definition.style.board).fixtures;
    const edges = (kind) => slots.filter((slot) => slot.kind === kind).map((slot) => [slot.at.x - 0.25, slot.at.x + 0.25]);
    const goals = edges('goal-note');
    const firstCard = Math.min(...edges('card-note').map(([left]) => left));
    assert.ok(goals.every(([, right]) => right < firstCard), `${id}'s goal notes end before its first card note`);
    assert.ok(Math.max(...edges('card-note').map(([, right]) => right)) <= definition.board.length / 2, `${id}'s card notes stay on the board`);
    return [id, goals.length];
  }));
  assert.deepEqual(columns, { studio: 8, workshop: 8, hq: 6 });
});

test('SOFA_FACES_AWAY mutant: every sofa and the seats on it face the opening view, so its backrest stands behind the agents on it', () => {
  const toward = { x: Math.sin(DEFAULT_AZIMUTH), y: Math.cos(DEFAULT_AZIMUTH) };
  const step = { north: [0, -1], east: [1, 0], south: [0, 1], west: [-1, 0] };
  const facesView = (facing) => step[facing][0] * toward.x + step[facing][1] * toward.y > 0;
  const sofas = Object.fromEntries(OFFICES.map(({ id, definition }) => [id, definition.furniture
    .filter((item) => ['sofa', 'lounge-sofa', 'pallet-sofa'].includes(item.kind))
    .map((item) => {
      const facing = item.facing ?? 'south';
      const model = furnitureModel(item.kind);
      const print = footprintAt(item.at, model.width, model.depth, facing);
      const seats = definition.lounge.filter((seat) => seat.at.x > print.x && seat.at.x < print.x + print.width
        && seat.at.y > print.y && seat.at.y < print.y + print.height);
      return { kind: item.kind, facesView: facesView(facing), seats: seats.map((seat) => seat.facing === facing) };
    })]));
  const sofa = (kind) => ({ kind, facesView: true, seats: [true, true, true] });
  assert.deepEqual(sofas, { studio: [sofa('sofa')], workshop: [sofa('pallet-sofa')], hq: [sofa('lounge-sofa'), sofa('lounge-sofa')] });
});

test('agents past the last desk queue by the door in a line, each on a cell of its own', () => {
  for (const { id, capacity, definition } of OFFICES) {
    const plan = officePlan(definition);
    const rows = Array.from({ length: capacity + 3 }, (_, k) => ({ id: `01JBQ8XK2M3N4P5Q6R7S8T9V${String(k).padStart(2, '0')}` }));
    const desks = castFrom(rows, plan);
    assert.equal(desks.length, capacity + 3);
    assert.deepEqual(desks.slice(capacity).map((desk) => desk.foot), definition.door.queue.slice(0, 3).map((spot) => spot.at), id);
    assert.equal(new Set(desks.map((desk) => key({ x: Math.floor(desk.foot.x), y: Math.floor(desk.foot.y) }))).size, capacity + 3,
      `${id} puts no two agents on one cell`);
  }
});

test('the smallest office that seats the crew is chosen, and it says so when even HQ is full', () => {
  const picked = Object.fromEntries([0, 1, 8, 9, 12, 13, 16, 17, 30].map((crew) => [crew, chooseOffice(crew, null)]));
  assert.deepEqual(picked, {
    0: { id: 'studio', full: false },
    1: { id: 'studio', full: false },
    8: { id: 'studio', full: false },
    9: { id: 'workshop', full: false },
    12: { id: 'workshop', full: false },
    13: { id: 'hq', full: false },
    16: { id: 'hq', full: false },
    17: { id: 'hq', full: true },
    30: { id: 'hq', full: true },
  });
  assert.equal(fullNotice(16, picked[16], false), null);
  assert.equal(fullNotice(17, picked[17], false), 'The office is full. HQ has 16 desks for 17 agents. 1 agent has no desk.');
});

test('FULL_FIRST_SPOKEN, FULL_BEFORE_SNAPSHOT, FULL_SPOKEN_UNDRAWN and DESK_AGAIN_DROPPED mutants: each change of the full line is said once, but not the line the first live snapshot brings, nor one the office does not show', () => {
  const seventeen = 'The office is full. HQ has 16 desks for 17 agents. 1 agent has no desk.';
  const eighteen = 'The office is full. HQ has 16 desks for 18 agents. 2 agents have no desk.';
  const connecting = { phase: 'connecting', epoch: 1, snapshotEpoch: 0 };
  const live = { phase: 'live', epoch: 1, snapshotEpoch: 1 };
  const resyncing = { phase: 'resyncing', epoch: 2, snapshotEpoch: 1 };
  // The office mounts before any snapshot, with no crew yet, and the first snapshot brings a crew that is already full.
  const mounted = followFull(undefined, null, true, connecting);
  assert.deepEqual(mounted, { shown: undefined, heard: null }, 'rows no live snapshot confirms leave the line unset');
  assert.deepEqual(followFull(mounted.shown, seventeen, true, live), { shown: seventeen, heard: null },
    'a crew already full in the first live snapshot is shown, not announced');
  assert.deepEqual(followFull(undefined, null, true, live), { shown: null, heard: null });
  assert.deepEqual(followFull(seventeen, seventeen, true, live), { shown: seventeen, heard: null });
  assert.deepEqual(followFull(null, seventeen, true, live), { shown: seventeen, heard: seventeen });
  assert.deepEqual(followFull(seventeen, eighteen, true, live), { shown: eighteen, heard: eighteen });
  assert.deepEqual(followFull(eighteen, null, true, live), { shown: null, heard: 'Every agent has a desk again.' });
  assert.deepEqual(followFull(seventeen, null, true, resyncing), { shown: seventeen, heard: null }, 'a resync changes nothing before its snapshot');
  assert.deepEqual(followFull(null, seventeen, false, live), { shown: seventeen, heard: null }, 'an office that cannot be drawn shows no line, so it says none');
});

test('a pin wins in both directions, and a smaller pin than the crew says the office is full', () => {
  assert.deepEqual(chooseOffice(2, 'hq'), { id: 'hq', full: false });
  assert.deepEqual(chooseOffice(9, 'workshop'), { id: 'workshop', full: false });
  assert.deepEqual(chooseOffice(12, 'studio'), { id: 'studio', full: true });
  assert.deepEqual(chooseOffice(17, 'workshop'), { id: 'workshop', full: true });
  assert.equal(fullNotice(12, chooseOffice(12, 'studio'), true),
    'Studio is pinned in Settings. It has 8 desks for 12 agents. 4 agents have no desk.');
});

test('an unknown stored pin reads as Automatic, and a pin survives a reload', () => {
  const values = new Map();
  const local = () => ({ getItem: (name) => values.get(name) ?? null, setItem: (name, value) => values.set(name, value) });
  assert.equal(readOfficePin(local), null, 'nothing stored is Automatic');
  writeOfficePin('workshop', local);
  assert.equal(readOfficePin(local), 'workshop');
  writeOfficePin(null, local);
  assert.equal(readOfficePin(local), null);
  for (const stored of ['penthouse', 'HQ', '', '{"id":"hq"}', 'automatic']) {
    values.set('dudes.floor.office-layout', stored);
    assert.equal(readOfficePin(local), null, `${JSON.stringify(stored)} reads as Automatic`);
    assert.deepEqual(chooseOffice(9, readOfficePin(local)), { id: 'workshop', full: false });
  }
  assert.equal(readOfficePin(() => { throw new Error('storage refused'); }), null);
});
