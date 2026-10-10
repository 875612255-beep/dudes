// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Four-neighbour breadth-first search over the studio grid.
//
// Walkability is a callback rather than a grid so the search never has to know
// how the map stores collision; the map module supplies one and a test supplies
// another. Pure: no canvas, no clock, no allocation the caller cannot see.

export interface GridPoint {
  readonly x: number;
  readonly y: number;
}

export interface GridSize {
  readonly width: number;
  readonly height: number;
}

/** True when a character may stand on this cell. */
export type Walkable = (x: number, y: number) => boolean;

/**
 * Neighbour order is fixed and part of the contract: up, right, down, left.
 * BFS returns one of possibly several shortest paths, and which one it returns
 * is decided here. Fixing it makes a path assertable exactly rather than only
 * checkable for validity, and keeps a character's route stable between frames.
 */
export const NEIGHBOURS: readonly GridPoint[] = Object.freeze([
  Object.freeze({ x: 0, y: -1 }),
  Object.freeze({ x: 1, y: 0 }),
  Object.freeze({ x: 0, y: 1 }),
  Object.freeze({ x: -1, y: 0 }),
]);

function inBounds(x: number, y: number, size: GridSize): boolean {
  return Number.isInteger(x) && Number.isInteger(y)
    && x >= 0 && y >= 0 && x < size.width && y < size.height;
}

/**
 * The shortest four-neighbour path from `start` to `goal`, inclusive of both.
 *
 * Returns `[start]` when start and goal are the same cell, and `null` when
 * either endpoint is off the grid or unwalkable, or when no route exists.
 */
export function findPath(
  start: GridPoint,
  goal: GridPoint,
  walkable: Walkable,
  size: GridSize,
): GridPoint[] | null {
  if (!Number.isInteger(size.width) || !Number.isInteger(size.height)
    || size.width <= 0 || size.height <= 0) {
    throw new Error(`grid size must be positive integers, received ${size.width}x${size.height}`);
  }
  if (!inBounds(start.x, start.y, size) || !inBounds(goal.x, goal.y, size)) return null;
  if (!walkable(start.x, start.y) || !walkable(goal.x, goal.y)) return null;
  if (start.x === goal.x && start.y === goal.y) return [{ x: start.x, y: start.y }];

  const cells = size.width * size.height;
  const index = (x: number, y: number): number => y * size.width + x;
  // -1 unvisited, -2 the root. Any other value is the index it was reached from.
  const cameFrom = new Int32Array(cells).fill(-1);
  const queue = new Int32Array(cells);
  let head = 0;
  let tail = 0;

  const startIndex = index(start.x, start.y);
  const goalIndex = index(goal.x, goal.y);
  cameFrom[startIndex] = -2;
  queue[tail++] = startIndex;

  while (head < tail) {
    const at = queue[head++];
    const ax = at % size.width;
    const ay = (at - ax) / size.width;
    for (const step of NEIGHBOURS) {
      const nx = ax + step.x;
      const ny = ay + step.y;
      if (!inBounds(nx, ny, size)) continue;
      const next = index(nx, ny);
      if (cameFrom[next] !== -1) continue;
      if (!walkable(nx, ny)) continue;
      cameFrom[next] = at;
      if (next === goalIndex) return reconstruct(cameFrom, startIndex, goalIndex, size.width);
      queue[tail++] = next;
    }
  }
  return null;
}

function reconstruct(
  cameFrom: Int32Array,
  startIndex: number,
  goalIndex: number,
  width: number,
): GridPoint[] {
  const reversed: GridPoint[] = [];
  let at = goalIndex;
  while (at !== startIndex) {
    const x = at % width;
    reversed.push({ x, y: (at - x) / width });
    at = cameFrom[at];
  }
  const x = startIndex % width;
  reversed.push({ x, y: (startIndex - x) / width });
  return reversed.reverse();
}
