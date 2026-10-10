// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Who is in the office: hires walk in through the door, retired agents walk
// out through it, and a first load or a resync places everyone at once.

import assert from 'node:assert/strict';
import test from 'node:test';

import { isBlocked, officePlan } from '../src/renderer/office3d/plan.ts';
import { STUDIO } from '../src/renderer/office3d/studio.ts';
import { castFrom } from '../src/renderer/scene/cast.ts';
import { charactersAt, planCast, planExit } from '../src/renderer/scene/motion.ts';
import { applyTransitions, arrivalTransitions, inOffice, rosterTransitions } from '../src/renderer/scene/presence.ts';

const ADA = '01JBQ8XK2M3N4P5Q6R7S8T9VWX';
const GRACE = '01JBQ8XK2M3N4P5Q6R7S8T9VWY';
const LIN = '01JBQ8XK2M3N4P5Q6R7S8T9VWZ';
const KAY = '01JBQ8XK2M3N4P5Q6R7S8T9VX0';
const NOW = 1_780_000_000_000;
const DOOR = { x: 0.5, y: 12.5 };
const plan = officePlan(STUDIO);
const working = (...ids) => ids.map((id) => ({ id, status: 'running' }));
const roster = (entries) => new Map(Object.entries(entries));

function walkable(point) {
  return !isBlocked(plan, Math.floor(point.x), Math.floor(point.y));
}

/** Two agents at their desks, then a hire on a delta, planned the way the office plans it. */
function hireLin() {
  const rows = working(ADA, GRACE);
  const desks = castFrom(rows, plan);
  const settled = planCast(rows, desks, plan, { update: 'snapshot', now: NOW });
  const changes = rosterTransitions(roster({ [ADA]: 'present', [GRACE]: 'present' }), [ADA, GRACE, LIN], 'delta');
  const next = working(ADA, GRACE, LIN);
  const motions = planCast(next, castFrom(next, plan, desks), plan, {
    previous: settled, update: 'delta', now: NOW,
    entering: new Set([...applyTransitions(roster({ [ADA]: 'present', [GRACE]: 'present' }), changes)]
      .flatMap(([id, presence]) => (presence === 'entering' ? [id] : []))),
  });
  return { changes, motions, lin: motions.find((motion) => motion.desk.agentId === LIN) };
}

test('HIRE_PLACED and DOOR_SKIPPED mutants: a hire enters at the door and walks to the first free desk', () => {
  const { changes, motions, lin } = hireLin();
  assert.deepEqual(changes, [{ id: LIN, from: null, to: 'entering' }]);
  assert.deepEqual(lin.desk.foot, { x: 10, y: 3.5 });
  assert.deepEqual(lin.travel[0], DOOR);
  assert.deepEqual(lin.travel.at(-1), { x: 10, y: 3.5 });
  for (const point of lin.travel.slice(0, -1)) assert.ok(walkable(point), JSON.stringify(point));
  const start = charactersAt([lin], NOW, false)[0];
  assert.deepEqual({ foot: start.foot, walking: start.walking }, { foot: DOOR, walking: true });
  const seated = charactersAt([lin], lin.arrivalAt, false)[0];
  assert.deepEqual({ foot: seated.foot, walking: seated.walking, seat: seated.seat }, { foot: { x: 10, y: 3.5 }, walking: false, seat: 0.46 });
  for (const motion of motions.filter((each) => each.desk.agentId !== LIN)) assert.equal(motion.travel.length, 1, 'no one else moves');

  const inside = applyTransitions(roster({ [ADA]: 'present', [GRACE]: 'present' }), changes);
  assert.deepEqual(arrivalTransitions(inside, (id) => id === LIN), [{ id: LIN, from: 'entering', to: 'present' }]);
  assert.deepEqual(arrivalTransitions(inside, () => false), [], 'still walking in');
});

test('RETIRE_VANISHES, EXIT_TELEPORT, SEAT_SHUFFLE and FREE_DESK mutants: a retired agent walks out and its desk is the next free one', () => {
  const rows = working(ADA, GRACE, LIN);
  const desks = castFrom(rows, plan);
  const settled = planCast(rows, desks, plan, { update: 'snapshot', now: NOW });
  const before = roster({ [ADA]: 'present', [GRACE]: 'present', [LIN]: 'present' });
  const changes = rosterTransitions(before, [ADA, LIN], 'delta');
  assert.deepEqual(changes, [{ id: GRACE, from: 'present', to: 'leaving' }]);

  const kept = working(ADA, LIN);
  const keptDesks = castFrom(kept, plan, desks);
  assert.deepEqual(keptDesks.map((desk) => desk.foot), [{ x: 4, y: 3.5 }, { x: 10, y: 3.5 }], 'Lin keeps her desk');
  const replanned = planCast(kept, keptDesks, plan, { previous: settled, update: 'delta', now: NOW });
  for (const motion of replanned) assert.equal(motion.travel.length, 1, 'no one else moves');

  const grace = charactersAt(settled, NOW, false).find((character) => character.agentId === GRACE);
  const exit = planExit(grace, plan, NOW, replanned);
  assert.deepEqual(exit.travel[0], { x: 7, y: 3.5 });
  assert.deepEqual(exit.travel.at(-1), DOOR);
  for (const point of exit.travel.slice(1)) assert.ok(walkable(point), JSON.stringify(point));
  assert.equal(charactersAt([exit], NOW + 420, false)[0].walking, true);
  assert.deepEqual(charactersAt([exit], exit.arrivalAt, false)[0].foot, DOOR);

  const leaving = applyTransitions(before, changes);
  const out = arrivalTransitions(leaving, (id) => id === GRACE);
  assert.deepEqual(out, [{ id: GRACE, from: 'leaving', to: 'gone' }]);
  assert.deepEqual([...applyTransitions(leaving, out)], [[ADA, 'present'], [LIN, 'present']]);

  const hired = working(ADA, LIN, KAY);
  assert.deepEqual(castFrom(hired, plan, keptDesks).map((desk) => desk.foot), [{ x: 4, y: 3.5 }, { x: 10, y: 3.5 }, { x: 7, y: 3.5 }],
    'the next hire takes the desk Grace left');
});

test('SNAPSHOT_PARADE mutant: a first load and a resync place everyone at once and let the departed go', () => {
  assert.deepEqual(rosterTransitions(new Map(), [ADA, GRACE], 'snapshot'),
    [{ id: ADA, from: null, to: 'present' }, { id: GRACE, from: null, to: 'present' }]);
  const rows = working(ADA, GRACE);
  const placed = planCast(rows, castFrom(rows, plan), plan, { update: 'snapshot', now: NOW, entering: new Set() });
  assert.deepEqual(charactersAt(placed, NOW, false).map(({ foot, walking }) => ({ foot, walking })),
    [{ foot: { x: 4, y: 3.5 }, walking: false }, { foot: { x: 7, y: 3.5 }, walking: false }]);

  assert.deepEqual(rosterTransitions(roster({ [ADA]: 'entering', [GRACE]: 'leaving', [LIN]: 'present' }), [ADA, LIN], 'snapshot'),
    [{ id: GRACE, from: 'leaving', to: 'gone' }, { id: ADA, from: 'entering', to: 'present' }]);
});

test('ENTERING_NEVER_LEAVES mutant: a hire and a retire in one update, and a hire retired before it reaches its desk', () => {
  const both = rosterTransitions(roster({ [ADA]: 'present', [GRACE]: 'present' }), [ADA, LIN], 'delta');
  assert.deepEqual(both, [{ id: GRACE, from: 'present', to: 'leaving' }, { id: LIN, from: null, to: 'entering' }]);
  assert.ok(both.every(({ id }) => id !== KAY), 'an agent hired and retired between two updates is never drawn');

  const { lin } = hireLin();
  const changes = rosterTransitions(roster({ [ADA]: 'present', [GRACE]: 'present', [LIN]: 'entering' }), [ADA, GRACE], 'delta');
  assert.deepEqual(changes, [{ id: LIN, from: 'entering', to: 'leaving' }]);
  const midway = charactersAt([lin], NOW + 1_000, false)[0];
  assert.equal(midway.walking, true);
  const exit = planExit(midway, plan, NOW + 1_000, []);
  assert.deepEqual(exit.travel[0], midway.foot, 'it turns back where it stands');
  assert.deepEqual(exit.travel.at(-1), DOOR);
  assert.ok(exit.travel.length < lin.travel.length);
});

test('PAUSE_RESYNC_DESK mutant: the snapshot after a resync leaves a paused agent where it stood', () => {
  const asking = [{ id: ADA, status: 'waiting', question: 'Which timeout is right?' }];
  const desks = castFrom(asking, plan);
  const lounge = planCast(asking, desks, plan, { update: 'snapshot', now: NOW });
  const paused = [{ ...asking[0], paused: true }];
  const [resynced] = planCast(paused, castFrom(paused, plan, desks), plan, { previous: lounge, update: 'snapshot', now: NOW + 1_000 });
  const kept = charactersAt([resynced], NOW + 1_000, false)[0];
  assert.deepEqual({ foot: kept.foot, seat: kept.seat, walking: kept.walking }, { foot: { x: 16.5, y: 8.5 }, seat: 0.53, walking: false });
  const [loaded] = planCast(paused, castFrom(paused, plan), plan, { update: 'snapshot', now: NOW });
  assert.deepEqual(charactersAt([loaded], NOW, false)[0].foot, { x: 4, y: 3.5 }, 'a first load knows no earlier spot, so the desk');
});

test('CROWD_TELEPORT mutant: a hire walks in even when wandering agents fill the routes', () => {
  const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  const crowd = Array.from({ length: 20 }, (_, k) => ({ id: `${ADA.slice(0, -2)}${alphabet[Math.floor(k / 32)]}${alphabet[k % 32]}`, status: 'idle' }));
  const desks = castFrom(crowd, plan);
  const settled = planCast(crowd, desks, plan, { update: 'snapshot', now: NOW });
  const next = [...crowd, { id: KAY, status: 'idle' }];
  const hire = planCast(next, castFrom(next, plan, desks), plan, { previous: settled, update: 'delta', now: NOW, entering: new Set([KAY]) })
    .find((motion) => motion.desk.agentId === KAY);
  assert.deepEqual(hire.travel[0], DOOR);
  assert.deepEqual(hire.travel.at(-1), hire.desk.foot);
  for (const point of hire.travel.slice(0, -1)) assert.ok(walkable(point), JSON.stringify(point));
  assert.equal(charactersAt([hire], hire.startedAt, false)[0].walking, true);
});

test('DESK_MOVED_RESYNC mutant: a paused agent without a desk takes the desk a resync freed rather than its old spot', () => {
  const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  const crew = Array.from({ length: 9 }, (_, k) => ({ id: `${ADA.slice(0, -2)}0${alphabet[k]}`, status: 'running', paused: k === 8 }));
  const desks = castFrom(crew, plan);
  assert.equal(plan.desks.includes(desks[8].seat), false, 'the ninth agent has no desk in an eight-desk office');
  const before = planCast(crew, desks, plan, { update: 'snapshot', now: NOW });
  const spot = charactersAt(before, NOW, false)[8].foot;
  const rest = crew.slice(1);
  const resynced = planCast(rest, castFrom(rest, plan, desks), plan, { previous: before, update: 'snapshot', now: NOW + 1_000 }).at(-1);
  assert.notDeepEqual(spot, { x: 4, y: 3.5 });
  const now = charactersAt([resynced], NOW + 1_000, false)[0];
  assert.deepEqual({ foot: now.foot, seat: now.seat }, { foot: { x: 4, y: 3.5 }, seat: 0.46 }, 'it sits at the first desk, which the first agent left');
});

test('EXIT_IGNORED and WAITING_HIRE_DRAWN mutants: a hire waits outside until a retiree has gone out of the door', () => {
  const rows = working(ADA, GRACE);
  const desks = castFrom(rows, plan);
  const settled = planCast(rows, desks, plan, { update: 'snapshot', now: NOW });
  const kept = working(ADA);
  const keptDesks = castFrom(kept, plan, desks);
  const after = planCast(kept, keptDesks, plan, { previous: settled, update: 'delta', now: NOW });
  const exit = planExit(charactersAt(settled, NOW, false)[1], plan, NOW, after);
  const hiredAt = exit.arrivalAt - 220;
  const next = working(ADA, KAY);
  const kay = planCast(next, castFrom(next, plan, keptDesks), plan, { previous: after, update: 'delta', now: hiredAt,
    entering: new Set([KAY]), passing: [exit] }).find((motion) => motion.desk.agentId === KAY);
  assert.deepEqual(kay.travel[0], DOOR);
  let closest = Infinity;
  for (let t = hiredAt; t <= kay.arrivalAt; t += 20) {
    if (t >= exit.arrivalAt || !inOffice('entering', kay, DOOR, t)) continue;
    const [gone, came] = charactersAt([exit, kay], t, false);
    closest = Math.min(closest, Math.hypot(gone.foot.x - came.foot.x, gone.foot.y - came.foot.y));
  }
  assert.equal(closest, Infinity, 'Grace is out of the door before Kay is drawn in it');
  assert.equal(inOffice('entering', kay, DOOR, kay.startedAt), true);
  assert.equal(inOffice('present', kay, DOOR, hiredAt), true, 'only a hire waits outside');
});

test('STEPPED_IN_HIDDEN mutant: a hire re-planned after it stepped in stays drawn where it stands', () => {
  const rows = working(ADA, GRACE);
  const desks = castFrom(rows, plan);
  const settled = planCast(rows, desks, plan, { update: 'snapshot', now: NOW });
  const next = working(ADA, GRACE, KAY);
  const nextDesks = castFrom(next, plan, desks);
  const hired = planCast(next, nextDesks, plan, { previous: settled, update: 'delta', now: NOW, entering: new Set([KAY]) });
  const kay = hired.find((motion) => motion.desk.agentId === KAY);
  const at = NOW + 3_600;
  const stood = charactersAt([kay], at, false)[0].foot;
  const replanned = planCast([{ id: ADA, status: 'waiting' }, ...working(GRACE, KAY)], nextDesks, plan,
    { previous: hired, update: 'delta', now: at, entering: new Set([KAY]) }).find((motion) => motion.desk.agentId === KAY);
  assert.deepEqual({ from: replanned.travel[0], holds: replanned.startedAt - at }, { from: stood, holds: 420 },
    'Ada\'s walk to the lounge holds Kay where she stands, past the door');
  assert.equal(inOffice('entering', replanned, DOOR, at), true, 'Kay stays drawn while she waits for Ada to pass');
  assert.deepEqual(charactersAt([replanned], at, false)[0].foot, stood);
  assert.equal(inOffice('entering', kay, DOOR, NOW - 1), false, 'a hire whose walk still starts at the door is outside');
});
