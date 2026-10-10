// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The offices the Floor can draw, smallest first, and which one it draws for
// a crew: the smallest that seats everyone, unless this browser pins one.

import { HQ } from './hq.ts';
import type { OfficeLayoutDefinition } from './layout.ts';
import { STUDIO } from './studio.ts';
import { WORKSHOP } from './workshop.ts';

export type LayoutId = 'studio' | 'workshop' | 'hq';

export interface Office {
  readonly id: LayoutId;
  /** How many agents get a desk. */
  readonly capacity: number;
  readonly definition: OfficeLayoutDefinition;
}

export const OFFICES: readonly Office[] = [
  { id: 'studio', capacity: 8, definition: STUDIO },
  { id: 'workshop', capacity: 12, definition: WORKSHOP },
  { id: 'hq', capacity: 16, definition: HQ },
];

export interface OfficeChoice {
  readonly id: LayoutId;
  /** More agents than desks: the rest queue by the door, then stand in the overflow area. */
  readonly full: boolean;
}

export function isLayoutId(value: unknown): value is LayoutId {
  return OFFICES.some((office) => office.id === value);
}

export function officeOf(id: LayoutId): Office {
  return OFFICES.find((office) => office.id === id) ?? OFFICES[0];
}

/** The pinned office, or else the smallest that seats the crew, or else the largest. */
export function chooseOffice(crew: number, pin: LayoutId | null): OfficeChoice {
  const office = OFFICES.find((entry) => entry.id === pin)
    ?? OFFICES.find((entry) => entry.capacity >= crew) ?? OFFICES[OFFICES.length - 1];
  return { id: office.id, full: crew > office.capacity };
}

/** What the office says when the crew outnumbers its desks, or `null` when everyone has one. */
export function fullNotice(crew: number, choice: OfficeChoice, pinned: boolean): string | null {
  if (!choice.full) return null;
  const { capacity, definition: { label } } = officeOf(choice.id);
  const spare = crew - capacity;
  const rest = `${capacity} desks for ${crew} agents. ${spare === 1 ? '1 agent has' : `${spare} agents have`} no desk.`;
  return pinned ? `${label} is pinned in Settings. It has ${rest}` : `The office is full. ${label} has ${rest}`;
}

/**
 * What the office's hidden status line says when the full line changes: the
 * new line, or that every agent has a desk again. `shown` stays `undefined`
 * until the first confirmed live snapshot, so the line that snapshot brings is
 * shown without a word. Nothing either for an unchanged line, for rows no
 * snapshot of a live stream confirms, or while the office cannot be drawn and
 * shows no line.
 */
export function followFull(shown: string | null | undefined, next: string | null, drawn: boolean,
  floor: { readonly phase: string; readonly epoch: number; readonly snapshotEpoch: number }): { readonly shown: string | null | undefined; readonly heard: string | null } {
  if (floor.phase !== 'live' || floor.snapshotEpoch !== floor.epoch || next === shown) return { shown, heard: null };
  return { shown: next, heard: shown === undefined || !drawn ? null : next ?? 'Every agent has a desk again.' };
}
