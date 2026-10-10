// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Any layout, turned into boxes in scene units: the floor, the walls, every
// model placed and turned, and the fixtures the renderer animates. Scene `x`
// is the plan's `x` and scene `z` the plan's `y`, both centred on the floor.

import type { Box, Fixture, Model, Vec3 } from './catalog.ts';
import {
  BELL, CHAIR, DOOR, TURN, boardModel, decorModel, deskModel, furnitureModel, turn, turnBox,
} from './catalog.ts';
import type { NoteAnchors, NotePlace } from '../scene/cardNotes.ts';
import type { FloorPatch, OfficeLayoutDefinition, Partition, PlanPoint, Wall } from './layout.ts';
import { deskCentre, footprintAt } from './plan.ts';

export interface PlacedFixture extends Fixture {
  /** The desk a desk fixture belongs to. */
  readonly desk?: number;
  /** The turn about `y` of the model it came from. */
  readonly turn: number;
}

export interface Furnished {
  readonly boxes: readonly Box[];
  readonly fixtures: readonly PlacedFixture[];
}

const TILE_TONES = ['#e9e4da', '#c9d3d6'] as const;
const EDGE = '#8a7a61';

function box(w: number, h: number, d: number, colour: string, x: number, y: number, z: number): Box {
  return { colour, size: { x: w, y: h, z: d }, at: { x, y, z } };
}

export function scenePoint(layout: Pick<OfficeLayoutDefinition, 'width' | 'depth'>, point: PlanPoint, y = 0): Vec3 {
  return { x: point.x - layout.width / 2, y, z: point.y - layout.depth / 2 };
}

function place(model: Model, angle: number, at: Vec3, desk?: number): Furnished {
  return {
    boxes: model.boxes.map((b) => turnBox(b, angle, at)),
    fixtures: model.fixtures.map((fixture) => {
      const turned = turn(fixture.at, angle);
      return {
        kind: fixture.kind,
        at: { x: turned.x + at.x, y: turned.y + at.y, z: turned.z + at.z },
        ...(fixture.box === undefined ? {} : { box: turnBox(fixture.box, angle, at) }),
        ...(desk === undefined ? {} : { desk }),
        turn: angle,
      };
    }),
  };
}

/** A wall model hangs on the north wall facing south, or on the west wall facing east. */
function hang(layout: OfficeLayoutDefinition, model: Model, wall: Wall, along: number): Furnished {
  return wall === 'north'
    ? place(model, TURN.south, { x: along - layout.width / 2, y: 0, z: -layout.depth / 2 })
    : place(model, TURN.east, { x: -layout.width / 2, y: 0, z: along - layout.depth / 2 });
}

function floorBoxes(layout: OfficeLayoutDefinition): readonly Box[] {
  const { width: w, depth: d, style: { floor } } = layout;
  if (floor.kind === 'planks') {
    return Array.from({ length: w }, (_, i) => box(1, 0.3, d, floor.tones[i % 2], -w / 2 + 0.5 + i, -0.15, 0));
  }
  const size = floor.kind === 'slabs' ? 2 : 1;
  const boxes: Box[] = [];
  for (let j = 0; j < d / size; j += 1) {
    for (let i = 0; i < w / size; i += 1) {
      const tone = floor.kind === 'tiles' ? (i + j) % 2 : (i + j) % 3 === 0 ? 0 : (i * 7 + j * 3) % 4 === 0 ? 1 : 2;
      boxes.push(box(size, 0.3, size, floor.tones[tone] ?? floor.tones[0], -w / 2 + size * (i + 0.5), -0.15, -d / 2 + size * (j + 0.5)));
    }
  }
  return boxes;
}

function shell(layout: OfficeLayoutDefinition): readonly Box[] {
  const { width: w, depth: d, style } = layout;
  const { wall } = style;
  return [
    box(w + 0.6, 0.9, d + 0.6, style.base, 0, -0.75, 0),
    box(w + 0.6, 0.12, d + 0.6, EDGE, 0, -1.2, 0),
    ...floorBoxes(layout),
    box(w + 0.4, 3.4, 0.4, wall.colour, 0, 1.55, -d / 2 - 0.2),
    box(0.4, 3.4, d + 0.4, wall.colour, -w / 2 - 0.2, 1.55, 0),
    ...(wall.kind === 'metal' ? [
      ...Array.from({ length: w * 2 }, (_, i) => box(0.12, 3.4, 0.08, wall.rib, -w / 2 + 0.25 + i * 0.5, 1.55, -d / 2)),
      ...Array.from({ length: d * 2 }, (_, i) => box(0.08, 3.4, 0.12, wall.rib, -w / 2, 1.55, -d / 2 + 0.25 + i * 0.5)),
    ] : []),
    box(w + 0.4, 0.25, 0.42, style.trim, 0, 0.12, -d / 2 - 0.18),
    box(0.42, 0.25, d + 0.4, style.trim, -w / 2 - 0.18, 0.12, 0),
  ];
}

/**
 * A glass partition through the middle of its cells. An end on the edge of
 * the floor runs on to that edge, so a run from a wall meets it, and an end
 * inside the floor stops at the middle of its cell, so two runs meet at a corner.
 */
function partition(layout: OfficeLayoutDefinition, run: Partition): Furnished {
  const [x0, x1] = [Math.min(run.from.x, run.to.x), Math.max(run.from.x, run.to.x)];
  const [y0, y1] = [Math.min(run.from.y, run.to.y), Math.max(run.from.y, run.to.y)];
  const start = (cell: number) => (cell === 0 ? 0 : cell + 0.5);
  const end = (cell: number, size: number) => (cell === size - 1 ? size : cell + 0.5);
  const across = y0 === y1;
  const from = across ? start(x0) : start(y0);
  const to = across ? end(x1, layout.width) : end(y1, layout.depth);
  const middle = scenePoint(layout, across ? { x: (from + to) / 2, y: y0 + 0.5 } : { x: x0 + 0.5, y: (from + to) / 2 });
  const sized = (length: number, height: number, thickness: number, colour: string, y: number) => (across
    ? box(length, height, thickness, colour, middle.x, y, middle.z) : box(thickness, height, length, colour, middle.x, y, middle.z));
  const pane = sized(to - from, 2.4, 0.08, '#bfe3f0', 1.2);
  return {
    boxes: [sized(to - from, 0.08, 0.12, '#9fb3bd', 2.42), sized(to - from, 0.06, 0.12, '#9fb3bd', 0.03)],
    fixtures: [{ kind: 'pane', at: pane.at, box: pane, turn: 0 }],
  };
}

function patch(layout: OfficeLayoutDefinition, area: FloorPatch): readonly Box[] {
  const centre = scenePoint(layout, { x: area.x + area.width / 2, y: area.y + area.height / 2 });
  if (area.kind === 'rug') {
    return [box(area.width, 0.32, area.height - 0.4, '#b7c9b0', centre.x, -0.13, centre.z),
      box(area.width - 0.6, 0.33, area.height - 1, '#c8d8c0', centre.x, -0.12, centre.z)];
  }
  if (area.kind === 'mat') return [box(area.width + 0.3, 0.32, area.height, '#9b6e57', centre.x, -0.13, centre.z)];
  const tiles: Box[] = [];
  for (let y = area.y; y < area.y + area.height; y += 1) {
    for (let x = area.x; x < area.x + area.width; x += 1) {
      const at = scenePoint(layout, { x: x + 0.5, y: y + 0.5 });
      tiles.push(box(1, 0.31, 1, TILE_TONES[(x + y) % 2], at.x, -0.14, at.z));
    }
  }
  return tiles;
}

/**
 * Where card notes hang: on the board's note slots, on the desk of the agent
 * that claimed them, and on the filing cabinet once done. Each place holds as
 * many notes as it has slots; a note past them reuses a slot to travel from,
 * and is not drawn.
 */
export function noteAnchors(furnished: Furnished, deskOf: ReadonlyMap<string, number>): NoteAnchors {
  const slots = (kind: PlacedFixture['kind'], desk?: number) => furnished.fixtures
    .filter((fixture) => fixture.kind === kind && (desk === undefined || fixture.desk === desk));
  const board = slots('card-note');
  const archive = slots('archive-slot');
  const slotsOf = (place: NotePlace): readonly PlacedFixture[] => place.kind === 'board' ? board
    : place.kind === 'archive' ? archive : slots('desk-note', deskOf.get(place.agentId));
  return {
    desks: new Set(deskOf.keys()),
    limit: (place) => slotsOf(place).length,
    at: (place, slot) => {
      const list = slotsOf(place);
      return list[slot % Math.max(1, list.length)]?.at ?? { x: 0, y: 0, z: 0 };
    },
  };
}

export function furnish(layout: OfficeLayoutDefinition): Furnished {
  const kindCounts = new Map<string, number>();
  const parts: Furnished[] = [
    { boxes: [...shell(layout), ...layout.floors.flatMap((area) => patch(layout, area))], fixtures: [] },
    ...layout.furniture.map((item) => {
      const variant = kindCounts.get(item.kind) ?? 0;
      kindCounts.set(item.kind, variant + 1);
      const model = furnitureModel(item.kind, variant);
      const facing = item.facing ?? 'south';
      const print = footprintAt(item.at, model.width, model.depth, facing);
      return place(model, TURN[facing], scenePoint(layout, print.centre));
    }),
    ...layout.desks.flatMap((seat, index) => [
      place(deskModel(index, layout.style.desk), TURN[seat.facing], scenePoint(layout, deskCentre(seat)), index),
      place({ width: 1, depth: 1, boxes: CHAIR, fixtures: [] }, TURN[seat.facing], scenePoint(layout, seat.at)),
    ]),
    place(BELL, TURN.south, scenePoint(layout, { x: layout.bell.at.x + 0.5, y: layout.bell.at.y + 0.5 })),
    ...layout.partitions.map((run) => partition(layout, run)),
    hang(layout, boardModel(layout.board.length, layout.style.board), layout.board.wall, layout.board.along),
    hang(layout, DOOR, layout.door.wall, layout.door.along),
    ...layout.decor.map((item) => hang(layout, decorModel(item.kind), item.wall, item.along)),
  ];
  return { boxes: parts.flatMap((part) => part.boxes), fixtures: parts.flatMap((part) => part.fixtures) };
}
