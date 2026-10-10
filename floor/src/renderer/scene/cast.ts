// SPDX-License-Identifier: MIT OR Apache-2.0

import type { Cell, PlanPoint } from '../office3d/layout.ts';
import type { OfficePlan, StationSpot } from '../office3d/plan.ts';
import { centreOf, isBlocked } from '../office3d/plan.ts';
import { NEIGHBOURS } from './pathfinding.ts';

/**
 * The part of an `AgentView` a character is made from.
 *
 * The index signature preserves the rest of the projection row for labels.
 */
export interface AgentRow {
  readonly id: string;
  readonly [field: string]: unknown;
}

/**
 * Where one agent works: a desk seat while seats last, then a place in the
 * queue by the door, then one in the overflow area, otherwise a free cell.
 * Only a desk seat is in `plan.desks`.
 */
export interface Desk {
  readonly agentId: string;
  /** Plan units. */
  readonly foot: PlanPoint;
  readonly seat: StationSpot | null;
}

/**
 * Place every agent, in the order given.
 *
 * An agent keeps the desk seat it held in `previous`, so a retire moves no one
 * else. Every other agent takes the first free seat, then a place in the queue
 * by the door, then one in the overflow area, then one of the plan's
 * `standingCells`. That last is not pretty, and it is deliberately not a
 * refusal: a real daemon-backed agent the floor declined to draw would be a
 * row the operator cannot see, which is worse than an unglamorous spot.
 */
export function castFrom(agents: readonly AgentRow[], plan: OfficePlan, previous: readonly Desk[] = []): readonly Desk[] {
  const current = new Set(agents.map((agent) => agent.id));
  const held = new Map(previous.flatMap((desk) => desk.seat !== null && current.has(desk.agentId)
    && plan.desks.includes(desk.seat) ? [[desk.agentId, desk.seat] as const] : []));
  const seated = new Set(held.values());
  const free = [...plan.desks.filter((seat) => !seated.has(seat)), ...plan.queue, ...plan.overflow];
  let standing: StandingCells | undefined;
  const desks: Desk[] = [];
  let beyond = 0;
  for (const agent of agents) {
    const seat = held.get(agent.id) ?? free.shift();
    if (seat !== undefined) {
      desks.push(Object.freeze({ agentId: agent.id, foot: seat.spot, seat }));
      continue;
    }
    standing ??= standingCells(plan);
    const cell = standing.clear[beyond] ?? standing.rest[beyond - standing.clear.length];
    if (cell === undefined) break;
    beyond += 1;
    desks.push(Object.freeze({ agentId: agent.id, foot: centreOf(cell.x, cell.y), seat: null }));
  }
  return Object.freeze(desks);
}

/** Where agents past the overflow area stand: first the cells that keep everyone free to move, then the rest. */
export interface StandingCells {
  readonly clear: readonly Cell[];
  readonly rest: readonly Cell[];
}

const STANDING = new WeakMap<OfficePlan, StandingCells>();

/**
 * Where agents past the overflow area stand, in the order they take the cells:
 * nearest the overflow area first, by walking. A cell is `clear` when it is
 * outside the room the board hangs in, more than two cells from the board, the
 * door and its queue, and standing there walls off no lounge seat, kitchen,
 * board or bell spot, and leaves every agent at a desk, in the queue, in the
 * overflow area or on an earlier cell, itself included, a free cell to walk
 * out to, unless the layout walled that agent in already. An agent at a desk
 * blocks its seat's cell, as it does for the walks. The `rest` are the other
 * free cells, nearest first, for crews too large for the clear ones.
 */
export function standingCells(plan: OfficePlan): StandingCells {
  const known = STANDING.get(plan);
  if (known !== undefined) return known;
  const at = (x: number, y: number): number => y * plan.width + x;

  /** The walkable cells a walk from `from` reaches without crossing a cell `shut` marks, nearest first, and a mark on each. */
  function walkFrom(from: readonly Cell[], shut: Uint8Array): { readonly order: readonly Cell[]; readonly reached: Uint8Array } {
    const reached = new Uint8Array(plan.width * plan.height);
    const order = [...from];
    for (const cell of from) reached[at(cell.x, cell.y)] = 1;
    for (let next = 0; next < order.length; next += 1) {
      for (const step of NEIGHBOURS) {
        const x = order[next].x + step.x;
        const y = order[next].y + step.y;
        if (isBlocked(plan, x, y) || reached[at(x, y)] === 1 || shut[at(x, y)] === 1) continue;
        reached[at(x, y)] = 1;
        order.push({ x, y });
      }
    }
    return { order, reached };
  }

  const canLeave = (reached: Uint8Array, cell: Cell): boolean => NEIGHBOURS.some((step) =>
    !isBlocked(plan, cell.x + step.x, cell.y + step.y) && reached[at(cell.x + step.x, cell.y + step.y)] === 1);
  const kept = [...plan.places.board, plan.door, ...plan.queue].map((station) => station.approach);
  const boardRoom = new Set(plan.boardRoom.map((cell) => at(cell.x, cell.y)));
  const offLimits = (cell: Cell): boolean => boardRoom.has(at(cell.x, cell.y))
    || kept.some((other) => Math.max(Math.abs(other.x - cell.x), Math.abs(other.y - cell.y)) <= 2);
  const places = Object.values(plan.places).flat().map((station) => station.approach);
  const door = [plan.door.approach];
  const held = [...plan.desks, ...plan.queue, ...plan.overflow].map((station) => station.approach);
  const closed = new Uint8Array(plan.width * plan.height);
  for (const cell of held) closed[at(cell.x, cell.y)] = 1;

  const { order } = walkFrom(plan.overflow.length > 0 ? plan.overflow.map((station) => station.approach) : door,
    new Uint8Array(plan.width * plan.height));
  let reached = walkFrom(door, closed).reached;
  const walledIn = held.filter((cell) => !canLeave(reached, cell));
  const clear: Cell[] = [];
  for (const cell of order) {
    if (closed[at(cell.x, cell.y)] === 1 || offLimits(cell)) continue;
    closed[at(cell.x, cell.y)] = 1;
    const after = walkFrom(door, closed).reached;
    const sealed = places.some((place) => reached[at(place.x, place.y)] === 1 && after[at(place.x, place.y)] !== 1);
    const boxed = [...held, cell].some((other) => !walledIn.includes(other) && !canLeave(after, other));
    if (sealed || boxed) {
      closed[at(cell.x, cell.y)] = 0;
      continue;
    }
    held.push(cell);
    clear.push(cell);
    reached = after;
  }
  const cells = Object.freeze({ clear: Object.freeze(clear), rest: Object.freeze(order.filter((cell) => closed[at(cell.x, cell.y)] !== 1)) });
  STANDING.set(plan, cells);
  return cells;
}
