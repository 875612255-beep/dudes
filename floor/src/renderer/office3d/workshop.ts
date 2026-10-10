// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The workshop: concrete and ribbed metal walls with a roll-up door, three
// pods of workbenches, a chalkboard goal wall on the west, the kitchen along
// the north, a ping-pong table, a pallet sofa with bean bags and the shipping
// corner in the east, and the door in the south-west. The sofa faces south, so
// its backrest stands behind the agents on it from the opening view.

import type { OfficeLayoutDefinition } from './layout.ts';
import { around, pod } from './layout.ts';

const PODS = [{ x: 5, y: 5 }, { x: 11, y: 5 }, { x: 8, y: 11 }] as const;

export const WORKSHOP: OfficeLayoutDefinition = {
  label: 'Workshop',
  width: 24,
  depth: 16,
  style: {
    floor: { kind: 'slabs', tones: ['#bdb9b1', '#aeaaa2', '#b6b2aa'] },
    wall: { kind: 'metal', colour: '#9aa3ab', rib: '#8a939b' },
    base: '#8f8a80',
    trim: '#6b747c',
    backdrop: { day: '#e9e6df', night: '#1b2028' },
    desk: { top: '#c9a26b', legs: '#5d6b75', toolbox: true },
    board: { frame: '#8a6a4f', face: '#2f3a36' },
  },
  desks: PODS.flatMap(pod),
  board: {
    wall: 'west',
    along: 10.5,
    length: 5,
    spots: [9.5, 10.5, 11.5].map((y) => ({ at: { x: 1.5, y }, facing: 'west' as const })),
  },
  lounge: [
    ...[19, 20, 21].map((x) => ({ at: { x: x + 0.5, y: 13.5 }, facing: 'south' as const, approach: { x, y: 14 }, height: 0.7 })),
    { at: { x: 18.5, y: 9.5 }, facing: 'west', approach: { x: 17, y: 9 }, height: 0.4 },
    { at: { x: 19.5, y: 10.5 }, facing: 'north', approach: { x: 19, y: 9 }, height: 0.4 },
    { at: { x: 18.5, y: 11.5 }, facing: 'west', approach: { x: 17, y: 11 }, height: 0.4 },
  ],
  bell: {
    at: { x: 22, y: 8 },
    spots: [
      { at: { x: 22.5, y: 9.5 }, facing: 'north' },
      { at: { x: 21.5, y: 8.5 }, facing: 'east' },
      { at: { x: 23.5, y: 8.5 }, facing: 'west' },
    ],
  },
  door: {
    wall: 'west',
    along: 14.5,
    spot: { at: { x: 0.5, y: 14.5 }, facing: 'east' },
    queue: [1, 2, 3, 4, 5, 6].map((x) => ({ at: { x: x + 0.5, y: 15.5 }, facing: 'west' as const })),
  },
  meetings: [{ at: { x: 19.5, y: 6 }, spots: around({ x: 18, y: 5, width: 3, height: 2 }) }],
  overflow: { x: 9, y: 1, width: 8, height: 1, facing: 'south' },
  kitchen: { x: 1, y: 1, width: 6, height: 1, facing: 'north' },
  furniture: [
    { kind: 'server-rack', at: { x: 0, y: 0 } },
    { kind: 'kitchen-counter', at: { x: 1, y: 0 } },
    { kind: 'fridge', at: { x: 6, y: 0 } },
    { kind: 'filing-cabinet', at: { x: 7, y: 0 } },
    { kind: 'crate-shelf', at: { x: 21, y: 0 } },
    ...PODS.map((at) => ({ kind: 'desk-divider' as const, at: { x: at.x + 1, y: at.y } })),
    { kind: 'plant', at: { x: 0, y: 6 } },
    { kind: 'plant', at: { x: 23, y: 4 } },
    { kind: 'ping-pong-table', at: { x: 18, y: 5 } },
    { kind: 'shipping-shelf', at: { x: 22, y: 11 } },
    { kind: 'bean-bag', at: { x: 18, y: 9 } },
    { kind: 'bean-bag', at: { x: 19, y: 10 } },
    { kind: 'bean-bag', at: { x: 18, y: 11 } },
    { kind: 'pallet-sofa', at: { x: 19, y: 13 }, facing: 'south' },
    { kind: 'plant', at: { x: 16, y: 15 } },
  ],
  partitions: [],
  decor: [
    { kind: 'pegboard', wall: 'west', along: 3.8 },
    { kind: 'neon-strip', wall: 'west', along: 4.5 },
    { kind: 'strip-window', wall: 'north', along: 6 },
    { kind: 'clock', wall: 'north', along: 8.5 },
    { kind: 'strip-window', wall: 'north', along: 11 },
    { kind: 'roll-up-door', wall: 'north', along: 16.5 },
    { kind: 'strip-window', wall: 'north', along: 22 },
  ],
  floors: [{ kind: 'mat', x: 0, y: 14, width: 1, height: 1 }],
  lamps: [
    { x: 7, y: 6, height: 2.4 },
    { x: 13, y: 6, height: 2.4 },
    { x: 10, y: 12, height: 2.4 },
    { x: 20.5, y: 13, height: 2.0 },
  ],
};
