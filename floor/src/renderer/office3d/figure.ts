// SPDX-License-Identifier: MIT OR Apache-2.0
//
// One crew figure, as boxes on five pivots: the body, two legs and two arms.
// Drawn facing south (`+z`), feet on the floor, about 1.6 units tall. Pure
// data, so a test can read which look and accessory a figure wears.

import type { Accessory, Look } from '../appearance.ts';
import type { Box, Vec3 } from './catalog.ts';

export type Limb = 'body' | 'leg-left' | 'leg-right' | 'arm-left' | 'arm-right';

export interface FigurePart {
  readonly limb: Limb;
  readonly box: Box;
  /** Parts with an outline get a thin dark cut edge around them. */
  readonly outline: boolean;
}

/** Where each limb turns, relative to the figure's feet. */
export const PIVOTS: Readonly<Record<Exclude<Limb, 'body'>, Vec3>> = Object.freeze({
  'leg-left': { x: -0.1, y: 0.55, z: 0 },
  'leg-right': { x: 0.1, y: 0.55, z: 0 },
  'arm-left': { x: -0.29, y: 1.0, z: 0 },
  'arm-right': { x: 0.29, y: 1.0, z: 0 },
});

const SHOE = '#2d2a28';
const EYE = '#1d2329';
const MOUTH = '#b5745a';
const GEAR = '#2d3640';
const LENS = '#bcd8e6';
const CAP = '#e0a12c';

function part(limb: Limb, w: number, h: number, d: number, colour: string, x: number, y: number, z: number, outline = true): FigurePart {
  return { limb, box: { colour, size: { x: w, y: h, z: d }, at: { x, y, z } }, outline };
}

export function figureParts(look: Look, accessory: Accessory | null): readonly FigurePart[] {
  const parts: FigurePart[] = [];
  for (const leg of ['leg-left', 'leg-right'] as const) {
    parts.push(part(leg, 0.17, 0.5, 0.2, look.pants, 0, -0.25, 0), part(leg, 0.18, 0.1, 0.27, SHOE, 0, -0.5, 0.03));
  }
  for (const arm of ['arm-left', 'arm-right'] as const) {
    parts.push(part(arm, 0.13, 0.4, 0.16, look.shirt, 0, -0.19, 0), part(arm, 0.12, 0.1, 0.14, look.skin, 0, -0.43, 0));
  }
  parts.push(
    part('body', 0.44, 0.48, 0.26, look.shirt, 0, 0.8, 0),
    part('body', 0.4, 0.38, 0.38, look.skin, 0, 1.25, 0),
    part('body', 0.42, 0.12, 0.4, look.hair, 0, 1.46, 0),
    look.style === 'long'
      ? part('body', 0.42, 0.44, 0.08, look.hair, 0, 1.24, -0.18)
      : part('body', 0.42, 0.26, 0.08, look.hair, 0, 1.33, -0.17),
    part('body', 0.06, 0.06, 0.02, EYE, -0.09, 1.27, 0.195, false),
    part('body', 0.06, 0.06, 0.02, EYE, 0.09, 1.27, 0.195, false),
    part('body', 0.1, 0.03, 0.02, MOUTH, 0, 1.15, 0.195, false),
  );
  if (look.style === 'bun') parts.push(part('body', 0.18, 0.14, 0.18, look.hair, 0, 1.58, -0.08));
  if (accessory === 'headphones') {
    parts.push(part('body', 0.46, 0.05, 0.1, GEAR, 0, 1.53, 0),
      part('body', 0.07, 0.15, 0.15, GEAR, -0.23, 1.27, 0), part('body', 0.07, 0.15, 0.15, GEAR, 0.23, 1.27, 0));
  } else if (accessory === 'glasses') {
    parts.push(part('body', 0.36, 0.04, 0.02, GEAR, 0, 1.28, 0.205, false),
      part('body', 0.12, 0.1, 0.015, LENS, -0.09, 1.27, 0.2, false), part('body', 0.12, 0.1, 0.015, LENS, 0.09, 1.27, 0.2, false));
  } else if (accessory === 'cap') {
    parts.push(part('body', 0.44, 0.1, 0.42, CAP, 0, 1.52, 0), part('body', 0.3, 0.04, 0.18, CAP, 0, 1.47, 0.27));
  }
  return parts;
}
