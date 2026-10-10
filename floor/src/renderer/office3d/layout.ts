// SPDX-License-Identifier: MIT OR Apache-2.0
//
// An office layout, as data. A layout names its floor, its style, where the
// desks, the goal board, the lounge seats, the bell and the door are, and
// what furniture stands where. Everything else is derived from it: the
// walkable grid comes from the furniture's footprints (`plan.ts`) and the
// scene from the furniture's models (`catalog.ts`).
//
// Plan units are floor cells. The origin is the north-west corner of the
// floor, `x` runs east and `y` runs south; the north and west edges carry the
// walls.

/** Compass directions on the floor plan: north is toward the back wall. */
export const FACINGS = ['north', 'east', 'south', 'west'] as const;
export type Facing = (typeof FACINGS)[number];

/** A point on the floor plan, in cells. It may sit between cells. */
export interface PlanPoint {
  readonly x: number;
  readonly y: number;
}

/** One whole cell. */
export interface Cell {
  readonly x: number;
  readonly y: number;
}

/** Somewhere a character stands, facing one way. It must be on a walkable cell. */
export interface Spot {
  readonly at: PlanPoint;
  readonly facing: Facing;
}

/**
 * Somewhere a character sits. A seat inside a footprint, such as a sofa's,
 * is reached from its `approach` cell; a seat on a walkable cell is its own
 * approach. `height` is the seat's top above the floor.
 */
export interface Seat extends Spot {
  readonly approach?: Cell;
  readonly height?: number;
}

/** A point above the floor plan: `x` and `y` on the plan, `height` up. */
export interface PlanPoint3 extends PlanPoint {
  readonly height: number;
}

/**
 * Four desks pushed together, two facing two, whose north-west desk cell is `at`: two seats on the north side
 * facing south, then two on the south side facing north.
 */
export function pod(at: Cell): readonly Seat[] {
  return [
    { at: { x: at.x + 1, y: at.y - 0.5 }, facing: 'south' },
    { at: { x: at.x + 3, y: at.y - 0.5 }, facing: 'south' },
    { at: { x: at.x + 1, y: at.y + 2.5 }, facing: 'north' },
    { at: { x: at.x + 3, y: at.y + 2.5 }, facing: 'north' },
  ];
}

/** A rectangle of whole cells. */
export interface Area {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The standing spots around a table, facing it: the four sides first, then the corners, so a small group
 * takes the sides. The table's footprint is `area`; spots that land on something else are dropped by the plan.
 */
export function around(area: Area): readonly Spot[] {
  const { x, y, width, height } = area;
  const cells = (from: number, count: number) => Array.from({ length: count }, (_, k) => from + k);
  const spot = (cx: number, cy: number, facing: Facing): Spot => ({ at: { x: cx + 0.5, y: cy + 0.5 }, facing });
  return [
    ...cells(x, width).map((cx) => spot(cx, y - 1, 'south')),
    ...cells(x, width).map((cx) => spot(cx, y + height, 'north')),
    ...cells(y, height).map((cy) => spot(x - 1, cy, 'east')),
    ...cells(y, height).map((cy) => spot(x + width, cy, 'west')),
    spot(x - 1, y - 1, 'south'), spot(x + width, y - 1, 'south'),
    spot(x - 1, y + height, 'north'), spot(x + width, y + height, 'north'),
  ];
}

/** A table a group thread gathers round: where it stands, and the spots beside it. */
export interface MeetingTable {
  readonly at: PlanPoint;
  readonly spots: readonly Spot[];
}

export type FurnitureKind = 'server-rack' | 'printer' | 'filing-cabinet' | 'plant' | 'standing-table'
  | 'water-cooler' | 'kitchen-counter' | 'fridge' | 'shipping-shelf' | 'bookshelf' | 'sofa' | 'armchair'
  | 'coffee-table' | 'floor-lamp' | 'desk-divider' | 'crate-shelf' | 'ping-pong-table' | 'pallet-sofa' | 'bean-bag'
  | 'meeting-table' | 'reception-desk' | 'kitchen-bar' | 'lounge-sofa' | 'tree';

/** One piece of furniture, by the north-west cell of its footprint. */
export interface Furnishing {
  readonly kind: FurnitureKind;
  readonly at: Cell;
  /** The way its front faces. Models are drawn facing south. */
  readonly facing?: Facing;
}

export type Wall = 'north' | 'west';
export type DecorKind = 'window' | 'strip-window' | 'wide-window' | 'roll-up-door' | 'pegboard' | 'neon-strip'
  | 'poster-red' | 'poster-blue' | 'clock';

/** Something hung on a wall, centred `along` it in cells from the north-west corner. */
export interface WallDecor {
  readonly kind: DecorKind;
  readonly wall: Wall;
  readonly along: number;
}

export type FloorKind = 'tiles' | 'rug' | 'mat';

export interface FloorPatch extends Area {
  readonly kind: FloorKind;
}

/** Planks run north to south, tiles alternate like a chequerboard, and two-cell slabs mix three tones. */
export type FloorFinish = { readonly kind: 'planks' | 'tiles'; readonly tones: readonly [string, string] }
  | { readonly kind: 'slabs'; readonly tones: readonly [string, string, string] };

export type WallFinish = { readonly kind: 'plaster'; readonly colour: string }
  | { readonly kind: 'metal'; readonly colour: string; readonly rib: string };

/** Each office style names its finishes; a new floor or wall kind adds its boxes in `furnish.ts`. */
export interface OfficeStyle {
  readonly floor: FloorFinish;
  readonly wall: WallFinish;
  readonly base: string;
  readonly trim: string;
  readonly backdrop: { readonly day: string; readonly night: string };
  readonly desk: { readonly top: string; readonly legs: string; readonly toolbox: boolean };
  readonly board: { readonly frame: string; readonly face: string };
}

/** A straight run of glass partition through whole cells, from one cell to another in the same row or column. */
export interface Partition {
  readonly from: Cell;
  readonly to: Cell;
}

export interface OfficeLayoutDefinition {
  /** What the operator sees it called. */
  readonly label: string;
  /** In cells. */
  readonly width: number;
  readonly depth: number;
  readonly style: OfficeStyle;
  /** The desk seats, in the order agents take them. Each desk stands in front of its seat. */
  readonly desks: readonly Seat[];
  /** The goal board hangs on a wall; its spots are where an agent stands to point at it. */
  readonly board: { readonly wall: Wall; readonly along: number; readonly length: number; readonly spots: readonly Spot[] };
  /** The lounge seats, in the order they fill. */
  readonly lounge: readonly Seat[];
  /** The bell's pedestal stands on `at`; its spots are where an agent rings it from. */
  readonly bell: { readonly at: Cell; readonly spots: readonly Spot[] };
  /** `queue` is where agents past the last desk wait, nearest the door first. */
  readonly door: { readonly wall: Wall; readonly along: number; readonly spot: Spot; readonly queue: readonly Spot[] };
  /** Where agents past the door queue stand, row by row, clear of the board and the door, so every seat and place stays reachable. */
  readonly overflow: Area & { readonly facing: Facing };
  /** Where a compacting agent stands, for coffee. */
  readonly kitchen: Area & { readonly facing: Facing };
  /** The tables a group thread gathers round, each with room for the largest thread of eight. */
  readonly meetings: readonly MeetingTable[];
  readonly furniture: readonly Furnishing[];
  readonly partitions: readonly Partition[];
  readonly decor: readonly WallDecor[];
  readonly floors: readonly FloorPatch[];
  /** Warm lights that come on at night. */
  readonly lamps: readonly PlanPoint3[];
}
