// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The studio: eight desks in two facing rows, the goal board on the west
// wall, the kitchen and the shipping corner along the north-east, the lounge
// in the south-east and the door in the south-west. The sofa faces south, so
// its backrest stands behind the agents on it from the opening view. The grid
// it makes is drawn in `tests/office-plan.test.mjs`.

import { around } from './layout.ts';
import type { OfficeLayoutDefinition } from './layout.ts';

export const STUDIO: OfficeLayoutDefinition = {
  label: 'Studio',
  width: 20,
  depth: 14,
  style: {
    floor: { kind: 'planks', tones: ['#cfb894', '#d8c3a1'] },
    wall: { kind: 'plaster', colour: '#efe6d6' },
    base: '#a89679',
    trim: '#c9b79c',
    backdrop: { day: '#efe8dc', night: '#1d2433' },
    desk: { top: '#f3ece0', legs: '#b8ad9b', toolbox: false },
    board: { frame: '#8a6a4f', face: '#c9a26b' },
  },
  desks: [
    ...[4, 7, 10, 13].map((x) => ({ at: { x, y: 3.5 }, facing: 'south' as const })),
    ...[4, 7, 10, 13].map((x) => ({ at: { x, y: 10.5 }, facing: 'north' as const })),
  ],
  board: {
    wall: 'west',
    along: 6.5,
    length: 5.2,
    spots: [5.5, 6.5, 7.5].map((y) => ({ at: { x: 1.5, y }, facing: 'west' as const })),
  },
  lounge: [
    ...[16, 17, 18].map((x) => ({ at: { x: x + 0.5, y: 8.5 }, facing: 'south' as const, approach: { x, y: 9 } })),
    { at: { x: 16.5, y: 12.5 }, facing: 'north', approach: { x: 16, y: 11 } },
  ],
  bell: {
    at: { x: 16, y: 4 },
    spots: [
      { at: { x: 16.5, y: 5.5 }, facing: 'north' },
      { at: { x: 15.5, y: 4.5 }, facing: 'east' },
      { at: { x: 17.5, y: 4.5 }, facing: 'west' },
    ],
  },
  door: {
    wall: 'west',
    along: 12.5,
    spot: { at: { x: 0.5, y: 12.5 }, facing: 'east' },
    queue: [1, 2, 3, 4, 5, 6].map((x) => ({ at: { x: x + 0.5, y: 13.5 }, facing: 'west' as const })),
  },
  meetings: [{
    at: { x: 1.5, y: 3.5 },
    spots: [...around({ x: 1, y: 3, width: 1, height: 1 }), { at: { x: 3.5, y: 3.5 }, facing: 'west' }],
  }],
  overflow: { x: 4, y: 1, width: 8, height: 1, facing: 'south' },
  kitchen: { x: 13, y: 1, width: 7, height: 2, facing: 'north' },
  furniture: [
    { kind: 'server-rack', at: { x: 0, y: 0 } },
    { kind: 'printer', at: { x: 2, y: 0 } },
    { kind: 'filing-cabinet', at: { x: 3, y: 0 } },
    { kind: 'plant', at: { x: 0, y: 2 } },
    { kind: 'standing-table', at: { x: 1, y: 3 } },
    { kind: 'water-cooler', at: { x: 13, y: 0 } },
    { kind: 'kitchen-counter', at: { x: 14, y: 0 } },
    { kind: 'fridge', at: { x: 19, y: 0 } },
    { kind: 'shipping-shelf', at: { x: 17, y: 3 } },
    { kind: 'plant', at: { x: 19, y: 3 } },
    { kind: 'bookshelf', at: { x: 18, y: 6 } },
    { kind: 'sofa', at: { x: 15, y: 8 }, facing: 'south' },
    { kind: 'coffee-table', at: { x: 16, y: 10 }, facing: 'east' },
    { kind: 'armchair', at: { x: 16, y: 12 }, facing: 'north' },
    { kind: 'floor-lamp', at: { x: 19, y: 13 } },
    { kind: 'plant', at: { x: 0, y: 9 } },
    { kind: 'plant', at: { x: 14, y: 13 } },
  ],
  partitions: [],
  decor: [
    { kind: 'window', wall: 'north', along: 5 },
    { kind: 'clock', wall: 'north', along: 6.75 },
    { kind: 'window', wall: 'north', along: 8.5 },
    { kind: 'window', wall: 'north', along: 12 },
    { kind: 'poster-red', wall: 'north', along: 15.5 },
    { kind: 'poster-blue', wall: 'north', along: 18.2 },
  ],
  floors: [
    { kind: 'tiles', x: 13, y: 0, width: 7, height: 3 },
    { kind: 'rug', x: 15, y: 8, width: 5, height: 5 },
    { kind: 'mat', x: 0, y: 12, width: 1, height: 1 },
  ],
  lamps: [
    { x: 7, y: 4.2, height: 1.6 },
    { x: 12.4, y: 9.8, height: 1.6 },
    { x: 19.2, y: 13, height: 2.0 },
  ],
};
