// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The furniture models, as boxes. Every model is drawn facing south, centred
// on its footprint, with `y` up from the floor; the footprint is what it
// blocks on the walkable grid. Wall models are drawn against the north wall,
// with `z` out from the wall's face. Pure data: no three.js here, so a test
// can hold each model's boxes to its footprint.

import type { DecorKind, Facing, FurnitureKind, OfficeStyle } from './layout.ts';

export interface Vec3 {
  readonly x: number;
  readonly y: number;
  readonly z: number;
}

export interface Box {
  readonly colour: string;
  readonly size: Vec3;
  readonly at: Vec3;
}

/** A part the renderer animates, lights or fills in, rather than batching with the static boxes. */
export type FixtureKind = 'server-light' | 'screen-line' | 'bulb' | 'steam' | 'clock-hand' | 'bell'
  | 'glass' | 'pane' | 'shelf-slot' | 'archive-slot' | 'desk-note' | 'goal-note' | 'card-note';

export interface Fixture {
  readonly kind: FixtureKind;
  readonly at: Vec3;
  /** The glowing or toggled box itself, when the fixture is one. */
  readonly box?: Box;
}

export interface Model {
  /** Footprint in cells, facing south: `width` across, `depth` front to back. */
  readonly width: number;
  readonly depth: number;
  readonly boxes: readonly Box[];
  readonly fixtures: readonly Fixture[];
}

function box(w: number, h: number, d: number, colour: string, x: number, y: number, z: number): Box {
  return { colour, size: { x: w, y: h, z: d }, at: { x, y, z } };
}

function glow(kind: FixtureKind, b: Box): Fixture {
  return { kind, at: b.at, box: b };
}

function slot(kind: FixtureKind, x: number, y: number, z: number): Fixture {
  return { kind, at: { x, y, z } };
}

const MUGS = ['#e2574c', '#3f7fd0', '#e0a12c', '#2f9b78'] as const;
const SCREEN = ['#9fd3ff', '#c3f0c8', '#f6d36b'] as const;
const SPINES = ['#e2574c', '#3f7fd0', '#e0a12c', '#2f9b78', '#8a62c9', '#f3ece0'] as const;
const PLANT_HEIGHTS = [0.6, 0.9, 0.4, 0.5] as const;
const BEAN_BAGS = ['#e07b39', '#3f7fd0', '#5c9c6a'] as const;
const PALLET_CUSHIONS = ['#e07b39', '#6a9fd8', '#e0a12c'] as const;
const TEAL_TOP = '#2e8b8b';

/** A desk, with its seated agent at `z = -SEAT_TO_DESK` facing `+z`. `variant` alternates the props. */
export const SEAT_TO_DESK = 0.88;

export function deskModel(variant: number, finish: OfficeStyle['desk']): Model {
  const boxes = [
    box(1.8, 0.1, 0.9, finish.top, 0, 0.78, 0),
    ...[[-0.8, -0.38], [0.8, -0.38], [-0.8, 0.38], [0.8, 0.38]].map(([x, z]) => box(0.08, 0.76, 0.08, finish.legs, x, 0.38, z)),
    box(0.66, 0.44, 0.06, '#2d3640', 0, 1.13, 0.18),
    box(0.1, 0.18, 0.1, '#5d6b75', 0, 0.88, 0.18),
    box(0.5, 0.03, 0.16, '#d8d3c8', 0, 0.845, -0.12),
    box(0.1, 0.12, 0.1, MUGS[variant % MUGS.length], 0.6, 0.89, -0.1),
  ];
  const fixtures: Fixture[] = SCREEN.map((colour, k) => glow('screen-line', box(0.3 - k * 0.06, 0.04, 0.01, colour, -0.1 + k * 0.03, 1.22 - k * 0.09, 0.145)));
  if (variant % 2 === 0) {
    boxes.push(box(0.18, 0.16, 0.18, '#c27c5c', -0.65, 0.91, 0.2), box(0.26, 0.26, 0.26, '#6fa36a', -0.65, 1.12, 0.2));
  } else {
    boxes.push(box(0.06, 0.4, 0.06, '#5d6b75', -0.7, 1.03, 0.25), box(0.22, 0.08, 0.16, '#f6d36b', -0.62, 1.24, 0.25));
    fixtures.push(glow('bulb', box(0.1, 0.05, 0.1, '#d8cfae', -0.62, 1.19, 0.25)));
  }
  if (finish.toolbox) boxes.push(box(0.5, 0.22, 0.3, '#e07b39', 0.55, 0.95, 0.25));
  for (let k = 0; k < 3; k += 1) fixtures.push(slot('desk-note', 0.3, 0.845 + k * 0.022, 0.08));
  return { width: 2, depth: 1, boxes, fixtures };
}

/** A desk chair, which stands on its seat's walkable cell and blocks nothing. */
export const CHAIR: readonly Box[] = [
  box(0.5, 0.08, 0.5, '#6f7d88', 0, 0.42, 0),
  box(0.5, 0.55, 0.08, '#6f7d88', 0, 0.72, -0.24),
  box(0.08, 0.38, 0.08, '#4a555e', 0, 0.2, 0),
  box(0.5, 0.05, 0.08, '#4a555e', 0, 0.03, 0),
  box(0.08, 0.05, 0.5, '#4a555e', 0, 0.03, 0),
];

export const BELL: Model = {
  width: 1,
  depth: 1,
  boxes: [box(0.7, 1.0, 0.7, '#c6bfae', 0, 0.5, 0)],
  fixtures: [slot('bell', 0, 1.0, 0)],
};

/** The bell's parts, relative to its fixture, so it can swing as one. */
export const BELL_PARTS: readonly Box[] = [
  box(0.5, 0.12, 0.5, '#b8862a', 0, 0.06, 0),
  box(0.4, 0.22, 0.4, '#d9a93a', 0, 0.22, 0),
  box(0.12, 0.12, 0.12, '#e8c260', 0, 0.38, 0),
];

function plant(height: number): Model {
  return {
    width: 1,
    depth: 1,
    boxes: [
      box(0.5, 0.5, 0.5, '#c27c5c', 0, 0.25, 0),
      box(0.12, height * 0.5, 0.12, '#6b7f4e', 0, 0.5 + height * 0.25, 0),
      box(0.8, 0.5, 0.8, '#6fa36a', 0, 0.7 + height * 0.5, 0),
      box(0.55, 0.45, 0.55, '#5b8f57', 0.15, 1.05 + height * 0.5, -0.1),
      box(0.4, 0.35, 0.4, '#7fb878', -0.15, 1.3 + height * 0.5, 0.1),
    ],
    fixtures: [],
  };
}

const MODELS: Readonly<Record<FurnitureKind, (variant: number) => Model>> = {
  'server-rack': () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.95, 2.3, 0.9, '#3d4650', 0, 1.0, 0)],
    fixtures: Array.from({ length: 18 }, (_, k) => glow('server-light',
      box(0.08, 0.06, 0.04, '#5fd08a', -0.14 + (k % 3) * 0.14, 0.35 + Math.floor(k / 3) * 0.3, 0.47))),
  }),
  printer: () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.95, 0.8, 0.7, '#d6d3cb', 0, 0.25, 0), box(0.9, 0.25, 0.6, '#b9b5ab', 0, 0.78, 0),
      box(0.7, 0.05, 0.5, '#ffffff', 0, 0.93, 0.05)],
    fixtures: [],
  }),
  'filing-cabinet': () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.7, 1.3, 0.6, '#a9b2b9', 0, 0.5, -0.05),
      ...[0.2, 0.6, 1.0].map((y) => box(0.3, 0.06, 0.04, '#7d878f', 0, y, 0.27))],
    fixtures: Array.from({ length: 6 }, (_, k) => slot('archive-slot', (k % 2) * 0.04 - 0.02, 1.17 + k * 0.025, -0.05)),
  }),
  plant: (variant) => plant(PLANT_HEIGHTS[variant % PLANT_HEIGHTS.length]),
  'standing-table': () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.5, 1.1, 0.5, '#9aa3ab', 0, 0.55, 0), box(0.8, 0.08, 0.6, '#f3ece0', 0, 1.12, 0)],
    fixtures: [],
  }),
  'water-cooler': () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.45, 1.2, 0.45, '#cfd8dc', 0, 0.45, 0), box(0.4, 0.5, 0.4, '#9fd3ff', 0, 1.3, 0)],
    fixtures: [],
  }),
  'kitchen-counter': () => ({
    width: 5,
    depth: 1,
    boxes: [
      box(4.6, 0.95, 0.8, '#d9cfbf', 0, 0.33, -0.1),
      box(4.7, 0.08, 0.9, '#f3ece0', 0, 0.84, -0.05),
      ...[-1.8, -0.6, 0.6, 1.8].map((x) => box(0.9, 0.7, 0.04, '#cbbfad', x, 0.4, 0.32)),
      box(0.45, 0.55, 0.4, '#4a3f38', -1.2, 1.15, -0.1),
      box(0.2, 0.12, 0.2, '#ffffff', -1.2, 0.94, 0.15),
    ],
    fixtures: [slot('steam', -1.2, 1.05, 0.15)],
  }),
  fridge: () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.95, 2.2, 0.8, '#e8e8e3', 0, 0.95, -0.05), box(0.06, 0.6, 0.06, '#9aa3ab', -0.38, 1.4, 0.37)],
    fixtures: [],
  }),
  'shipping-shelf': () => ({
    width: 2,
    depth: 1,
    boxes: [box(1.96, 0.08, 0.6, '#a07955', 0, 0.9, 0), box(1.96, 0.08, 0.6, '#a07955', 0, 1.6, 0),
      box(0.08, 1.7, 0.6, '#8a6a4f', -0.94, 0.85, 0), box(0.08, 1.7, 0.6, '#8a6a4f', 0.94, 0.85, 0)],
    fixtures: Array.from({ length: 10 }, (_, k) => slot('shelf-slot', -0.72 + (k % 5) * 0.36, 1.08 + Math.floor(k / 5) * 0.7, 0)),
  }),
  bookshelf: () => ({
    width: 2,
    depth: 1,
    boxes: [box(1.8, 1.9, 0.6, '#a07955', 0, 0.95, -0.1),
      ...Array.from({ length: 24 }, (_, k) => box(0.18, 0.32, 0.3, SPINES[(Math.floor(k / 6) + (k % 6)) % SPINES.length],
        -0.7 + (k % 6) * 0.27, 0.28 + Math.floor(k / 6) * 0.45, 0.07))],
    fixtures: [],
  }),
  sofa: () => ({
    width: 5,
    depth: 1,
    boxes: [
      box(3.4, 0.45, 0.9, '#d98f6f', 0, 0.22, 0.05),
      box(3.4, 0.9, 0.3, '#c97d5d', 0, 0.5, -0.3),
      box(0.3, 0.65, 1.0, '#c97d5d', -1.85, 0.38, 0),
      box(0.3, 0.65, 1.0, '#c97d5d', 1.85, 0.38, 0),
      ...[-1, 0, 1].map((x) => box(0.7, 0.1, 0.8, '#e7a585', x, 0.48, 0.05)),
    ],
    fixtures: [],
  }),
  armchair: () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.95, 0.55, 0.95, '#7d9c8a', 0, 0.27, 0), box(0.95, 0.7, 0.25, '#6d8a79', 0, 0.55, -0.35)],
    fixtures: [],
  }),
  'coffee-table': () => ({
    width: 1,
    depth: 2,
    boxes: [box(0.9, 0.36, 1.6, '#a07955', 0, 0.18, 0), box(0.2, 0.16, 0.2, '#f3ece0', -0.1, 0.44, -0.3),
      box(0.36, 0.06, 0.5, '#3f7fd0', 0.1, 0.39, 0.4)],
    fixtures: [],
  }),
  'floor-lamp': () => ({
    width: 1,
    depth: 1,
    boxes: [box(0.1, 1.7, 0.1, '#5d6b75', 0, 0.85, 0), box(0.5, 0.35, 0.5, '#f6e3b0', 0, 1.8, 0)],
    fixtures: [glow('bulb', box(0.3, 0.06, 0.3, '#d8cfae', 0, 1.6, 0))],
  }),
  /** The screen between the left and right desks of a pod, standing on the desks. */
  'desk-divider': () => ({
    width: 2,
    depth: 2,
    boxes: [box(0.06, 0.4, 1.8, '#c9d3d6', 0, 1.0, 0)],
    fixtures: [],
  }),
  'crate-shelf': () => ({
    width: 3,
    depth: 1,
    boxes: [
      ...[-1.2, 1.2].map((x) => box(0.08, 2.4, 0.7, '#6b747c', x, 1.2, 0)),
      ...[0, 1, 2].flatMap((shelf) => [box(2.4, 0.08, 0.7, '#8a939b', 0, 0.5 + shelf * 0.8, 0),
        ...[0, 1, 2, 3].map((k) => box(0.5, 0.4, 0.5, k % 2 === 0 ? '#d8b98a' : '#c9a774', -0.9 + k * 0.6, 0.75 + shelf * 0.8, 0))]),
    ],
    fixtures: [],
  }),
  'ping-pong-table': () => ({
    width: 3,
    depth: 2,
    boxes: [
      box(2.8, 0.1, 1.5, '#2f7a5a', 0, 0.8, 0),
      box(0.06, 0.2, 1.5, '#f3ece0', 0, 0.95, 0),
      ...[[-1.2, -0.6], [1.2, -0.6], [-1.2, 0.6], [1.2, 0.6]].map(([x, z]) => box(0.1, 0.78, 0.1, '#5d6b75', x, 0.39, z)),
      box(0.18, 0.03, 0.26, '#e2574c', -0.8, 0.865, 0.3),
      box(0.18, 0.03, 0.26, '#2d3640', 0.9, 0.865, -0.35),
      box(0.06, 0.06, 0.06, '#ffffff', 0.45, 0.88, 0.1),
    ],
    fixtures: [],
  }),
  'pallet-sofa': () => ({
    width: 3,
    depth: 1,
    boxes: [
      box(2.8, 0.3, 0.9, '#c9a26b', 0, 0.15, 0.05),
      box(2.8, 0.3, 0.9, '#b48a63', 0, 0.45, 0.05),
      box(2.8, 0.6, 0.25, '#c9a26b', 0, 0.9, -0.36),
      ...PALLET_CUSHIONS.map((colour, k) => box(0.8, 0.1, 0.6, colour, k - 1, 0.65, 0.1)),
    ],
    fixtures: [],
  }),
  'bean-bag': (variant) => {
    const colour = BEAN_BAGS[variant % BEAN_BAGS.length];
    return {
      width: 1,
      depth: 1,
      boxes: [box(0.86, 0.34, 0.86, colour, 0, 0.17, 0), box(0.86, 0.3, 0.3, colour, 0, 0.45, -0.28)],
      fixtures: [],
    };
  },
  'meeting-table': () => ({
    width: 3,
    depth: 1,
    boxes: [box(2.4, 0.1, 0.9, '#fafafa', 0, 0.76, 0),
      ...[[-1.05, -0.35], [1.05, -0.35], [-1.05, 0.35], [1.05, 0.35]].map(([x, z]) => box(0.1, 0.72, 0.1, '#9aa3ab', x, 0.36, z)),
      box(0.36, 0.02, 0.26, '#f6d36b', -0.6, 0.82, 0.1), box(0.36, 0.02, 0.26, '#9fd3c7', 0.5, 0.82, -0.1)],
    fixtures: [],
  }),
  'reception-desk': () => ({
    width: 3,
    depth: 1,
    boxes: [box(2.8, 1.0, 0.75, '#e9eef0', 0, 0.5, 0), box(2.94, 0.08, 0.86, TEAL_TOP, 0, 1.04, 0),
      box(0.5, 0.32, 0.05, '#2d3640', 0.6, 1.24, -0.2)],
    fixtures: [],
  }),
  /** A bar along the back of its footprint, with three stools in front. */
  'kitchen-bar': () => ({
    width: 4,
    depth: 2,
    boxes: [
      box(3.9, 1.0, 0.7, '#e9eef0', 0, 0.5, -0.5),
      box(3.96, 0.08, 0.8, TEAL_TOP, 0, 1.04, -0.5),
      ...[-1.5, 0, 1.5].flatMap((x) => [box(0.4, 0.1, 0.4, '#5d6b75', x, 0.68, 0.3), box(0.08, 0.66, 0.08, '#9aa3ab', x, 0.33, 0.3)]),
    ],
    fixtures: [],
  }),
  'lounge-sofa': () => ({
    width: 3,
    depth: 1,
    boxes: [
      box(2.84, 0.45, 0.9, '#6f9fa0', 0, 0.22, 0.05),
      box(2.84, 0.9, 0.3, '#556b6b', 0, 0.5, -0.3),
      ...[-1.42, 1.42].map((x) => box(0.16, 0.65, 1.0, '#556b6b', x, 0.38, 0)),
      ...[-1, 0, 1].map((x) => box(0.8, 0.1, 0.8, '#e7a585', x, 0.48, 0.05)),
    ],
    fixtures: [],
  }),
  tree: () => ({
    width: 2,
    depth: 2,
    boxes: [
      box(1.2, 0.6, 1.2, '#d9d4c8', 0, 0.3, 0),
      box(0.25, 1.6, 0.25, '#7a5a3e', 0, 1.4, 0),
      box(2.0, 1.0, 2.0, '#5b8f57', 0, 2.4, 0),
      box(1.5, 0.8, 1.5, '#6fa36a', 0.2, 3.1, -0.2),
      box(1.0, 0.6, 1.0, '#7fb878', -0.2, 3.6, 0.2),
    ],
    fixtures: [],
  }),
};

export function furnitureModel(kind: FurnitureKind, variant = 0): Model {
  return MODELS[kind](variant);
}

const DECOR: Readonly<Record<DecorKind, Model>> = {
  window: {
    width: 2.5,
    depth: 0,
    boxes: [box(2.4, 1.7, 0.1, '#e3d8c4', 0, 1.9, 0.02), box(0.08, 1.4, 0.14, '#e3d8c4', 0, 1.9, 0.07),
      box(2.5, 0.12, 0.3, '#e3d8c4', 0, 1.02, 0.1)],
    fixtures: [glow('glass', box(2.1, 1.4, 0.12, '#bcd8e6', 0, 1.9, 0.05))],
  },
  'strip-window': {
    width: 2.4,
    depth: 0,
    boxes: [box(2.3, 1.3, 0.1, '#6b747c', 0, 2.4, 0.02), box(0.07, 1.0, 0.14, '#6b747c', 0, 2.4, 0.07),
      box(2.4, 0.12, 0.3, '#6b747c', 0, 1.82, 0.1)],
    fixtures: [glow('glass', box(2.0, 1.0, 0.12, '#bcd8e6', 0, 2.4, 0.05))],
  },
  'wide-window': {
    width: 4,
    depth: 0,
    boxes: [box(3.9, 2.1, 0.1, '#c9d3d6', 0, 1.8, 0.02), ...[-0.6, 0.6].map((x) => box(0.07, 1.8, 0.14, '#c9d3d6', x, 1.8, 0.07)),
      box(4.0, 0.12, 0.3, '#c9d3d6', 0, 0.82, 0.1)],
    fixtures: [glow('glass', box(3.6, 1.8, 0.12, '#bcd8e6', 0, 1.8, 0.05))],
  },
  'roll-up-door': {
    width: 5,
    depth: 0,
    boxes: [box(5.0, 3.0, 0.2, '#c9cdd1', 0, 1.5, 0.05),
      ...Array.from({ length: 10 }, (_, k) => box(5.0, 0.06, 0.24, '#b1b6bb', 0, 0.2 + k * 0.29, 0.08))],
    fixtures: [],
  },
  pegboard: {
    width: 3.4,
    depth: 0,
    boxes: [box(3.4, 1.6, 0.1, '#c9a26b', 0, 1.7, 0.06),
      ...Array.from({ length: 9 }, (_, k) => box(0.12, 0.5, 0.12, ['#5d6b75', '#e07b39', '#9aa3ab'][k % 3],
        -1.2 + Math.floor(k / 3) * 1.2, 1.4 + (k % 3) * 0.5, 0.18))],
    fixtures: [],
  },
  'neon-strip': {
    width: 4,
    depth: 0,
    boxes: [box(4.0, 0.12, 0.12, '#ff8a3d', 0, 3.0, 0.1)],
    fixtures: [glow('bulb', box(3.8, 0.1, 0.1, '#ffb07a', 0, 3.0, 0.16))],
  },
  'poster-red': {
    width: 1.3,
    depth: 0,
    boxes: [box(1.3, 0.9, 0.08, '#e2574c', 0, 2.2, 0.03), box(1.1, 0.7, 0.1, '#f4ead8', 0, 2.2, 0.05)],
    fixtures: [],
  },
  'poster-blue': {
    width: 0.9,
    depth: 0,
    boxes: [box(0.9, 1.2, 0.08, '#3f7fd0', 0, 2.0, 0.03), box(0.7, 1.0, 0.1, '#f4ead8', 0, 2.0, 0.05)],
    fixtures: [],
  },
  clock: {
    width: 0.7,
    depth: 0,
    boxes: [box(0.7, 0.7, 0.08, '#f8f4ec', 0, 2.75, 0.03)],
    fixtures: [slot('clock-hand', 0, 2.75, 0.1)],
  },
};

export function decorModel(kind: DecorKind): Model {
  return DECOR[kind];
}

export const DOOR: Model = {
  width: 1.3,
  depth: 0,
  boxes: [box(1.3, 2.3, 0.12, '#8a6a4f', 0, 1.15, 0), box(1.1, 2.1, 0.14, '#b48a63', 0, 1.05, 0.02),
    box(0.12, 0.12, 0.16, '#d9a93a', 0.4, 1.05, 0.1)],
  fixtures: [],
};

/** How wide a note on the goal board is drawn. */
const BOARD_NOTE = 0.5;

/**
 * The goal board for a wall run of `length` cells: card notes on its right,
 * and on its left as many columns of goal notes as fit before the first card
 * column, up to four, so a short board never draws two notes in one place.
 */
export function boardModel(length: number, finish: OfficeStyle['board']): Model {
  const half = length / 2;
  const cards = [0, 1, 2].map((k) => 0.35 + k * 0.72);
  const goals = [0, 1, 2, 3].map((k) => -half + 0.45 + k * 0.62).filter((x) => x + BOARD_NOTE / 2 < cards[0] - BOARD_NOTE / 2);
  const rows = [2.25, 1.63];
  return {
    width: length,
    depth: 0,
    boxes: [box(length + 0.2, 2.0, 0.16, finish.frame, 0, 1.75, 0.05), box(length, 1.8, 0.18, finish.face, 0, 1.75, 0.08)],
    fixtures: [
      ...rows.flatMap((y) => goals.map((x) => slot('goal-note', x, y, 0.2))),
      ...rows.flatMap((y) => cards.map((x) => slot('card-note', x, y, 0.2))),
    ],
  };
}

/** Rotation about `y` that turns a south-facing model to face `facing`. */
export const TURN: Readonly<Record<Facing, number>> = Object.freeze({
  south: 0, east: Math.PI / 2, north: Math.PI, west: -Math.PI / 2,
});

/** Turn a local point about `y` by `angle`, as three.js does: `+z` turned by `π/2` becomes `+x`. */
export function turn(point: Vec3, angle: number): Vec3 {
  const cos = Math.round(Math.cos(angle) * 1e9) / 1e9;
  const sin = Math.round(Math.sin(angle) * 1e9) / 1e9;
  return { x: point.x * cos + point.z * sin, y: point.y, z: -point.x * sin + point.z * cos };
}

/** A box turned by a quarter turn, or a half: its size swaps across for a quarter. */
export function turnBox(b: Box, angle: number, offset: Vec3): Box {
  const at = turn(b.at, angle);
  const quarter = Math.abs(Math.round(Math.sin(angle))) === 1;
  return {
    colour: b.colour,
    size: quarter ? { x: b.size.z, y: b.size.y, z: b.size.x } : b.size,
    at: { x: at.x + offset.x, y: at.y + offset.y, z: at.z + offset.z },
  };
}
