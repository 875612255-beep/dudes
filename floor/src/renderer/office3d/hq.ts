// SPDX-License-Identifier: MIT OR Apache-2.0
//
// HQ: pale tiles and white walls, four pods of desks, a glass check room in
// the north-west holding the goal board, reception by the door in the west,
// a glass server room in the south-west, the kitchen and its bar in the
// north-east, and a lounge by an indoor tree in the south-east. Its two sofas
// face south and east, so from the opening view their backrests stand behind
// the agents on them.

import type { OfficeLayoutDefinition } from './layout.ts';
import { around, pod } from './layout.ts';

const PODS = [{ x: 7, y: 5 }, { x: 13, y: 5 }, { x: 7, y: 11 }, { x: 13, y: 11 }] as const;

export const HQ: OfficeLayoutDefinition = {
  label: 'HQ',
  width: 28,
  depth: 18,
  style: {
    floor: { kind: 'tiles', tones: ['#e7ecee', '#dde4e7'] },
    wall: { kind: 'plaster', colour: '#f4f6f6' },
    base: '#9aa6a8',
    trim: '#c9d3d6',
    backdrop: { day: '#eef1ef', night: '#18202c' },
    desk: { top: '#fafafa', legs: '#b8ad9b', toolbox: false },
    board: { frame: '#9fb3bd', face: '#f7fbfc' },
  },
  desks: PODS.flatMap(pod),
  board: {
    wall: 'north',
    along: 2.5,
    length: 4.2,
    spots: [1, 2, 3].map((x) => ({ at: { x: x + 0.5, y: 1.5 }, facing: 'north' as const })),
  },
  lounge: [
    ...[22, 23, 24].map((x) => ({ at: { x: x + 0.5, y: 10.5 }, facing: 'south' as const, approach: { x, y: 11 } })),
    ...[12, 13, 14].map((y) => ({ at: { x: 21.5, y: y + 0.5 }, facing: 'east' as const, approach: { x: 22, y } })),
  ],
  bell: {
    at: { x: 19, y: 8 },
    spots: [
      { at: { x: 19.5, y: 9.5 }, facing: 'north' },
      { at: { x: 18.5, y: 8.5 }, facing: 'east' },
      { at: { x: 20.5, y: 8.5 }, facing: 'west' },
    ],
  },
  door: {
    wall: 'west',
    along: 8.5,
    spot: { at: { x: 0.5, y: 8.5 }, facing: 'east' },
    queue: [
      ...[9, 10, 11, 12, 13].map((y) => ({ at: { x: 0.5, y: y + 0.5 }, facing: 'north' as const })),
      { at: { x: 1.5, y: 13.5 }, facing: 'west' },
    ],
  },
  meetings: [{ at: { x: 2.5, y: 3.5 }, spots: around({ x: 1, y: 3, width: 3, height: 1 }) }],
  overflow: { x: 7, y: 1, width: 10, height: 1, facing: 'south' },
  kitchen: { x: 21, y: 1, width: 7, height: 2, facing: 'north' },
  furniture: [
    { kind: 'meeting-table', at: { x: 1, y: 3 } },
    { kind: 'filing-cabinet', at: { x: 6, y: 0 } },
    { kind: 'plant', at: { x: 0, y: 6 } },
    { kind: 'reception-desk', at: { x: 2, y: 10 } },
    { kind: 'server-rack', at: { x: 1, y: 15 } },
    { kind: 'server-rack', at: { x: 2, y: 15 } },
    ...PODS.map((at) => ({ kind: 'desk-divider' as const, at: { x: at.x + 1, y: at.y } })),
    { kind: 'plant', at: { x: 17, y: 0 } },
    { kind: 'kitchen-counter', at: { x: 22, y: 0 } },
    { kind: 'fridge', at: { x: 27, y: 0 } },
    { kind: 'kitchen-bar', at: { x: 22, y: 3 } },
    { kind: 'shipping-shelf', at: { x: 21, y: 8 } },
    { kind: 'bookshelf', at: { x: 26, y: 7 } },
    { kind: 'lounge-sofa', at: { x: 22, y: 10 }, facing: 'south' },
    { kind: 'coffee-table', at: { x: 23, y: 12 }, facing: 'east' },
    { kind: 'lounge-sofa', at: { x: 21, y: 12 }, facing: 'east' },
    { kind: 'tree', at: { x: 26, y: 15 } },
    { kind: 'plant', at: { x: 16, y: 17 } },
  ],
  partitions: [
    { from: { x: 5, y: 0 }, to: { x: 5, y: 5 } },
    { from: { x: 0, y: 5 }, to: { x: 1, y: 5 } },
    { from: { x: 3, y: 5 }, to: { x: 5, y: 5 } },
    { from: { x: 0, y: 14 }, to: { x: 3, y: 14 } },
    { from: { x: 3, y: 14 }, to: { x: 3, y: 17 } },
  ],
  decor: [
    { kind: 'wide-window', wall: 'north', along: 8.5 },
    { kind: 'clock', wall: 'north', along: 11 },
    { kind: 'wide-window', wall: 'north', along: 13.5 },
    { kind: 'wide-window', wall: 'north', along: 18.5 },
    { kind: 'wide-window', wall: 'north', along: 24.5 },
  ],
  floors: [{ kind: 'mat', x: 0, y: 8, width: 1, height: 1 }],
  lamps: [
    { x: 9, y: 6, height: 2.6 },
    { x: 15, y: 6, height: 2.6 },
    { x: 9, y: 12, height: 2.6 },
    { x: 15, y: 12, height: 2.6 },
    { x: 23.5, y: 13, height: 2.2 },
  ],
};
