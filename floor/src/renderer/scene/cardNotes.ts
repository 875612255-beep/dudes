// SPDX-License-Identifier: MIT OR Apache-2.0

import type { CardRow } from '../board.ts';

export type NotePlace = { readonly kind: 'board' } | { readonly kind: 'desk'; readonly agentId: string }
  | { readonly kind: 'archive' };

/** Where a note hangs, in scene units. */
export interface NotePoint {
  readonly x: number;
  readonly y: number;
  readonly z: number;
}

export interface CardNote {
  readonly card: CardRow;
  readonly place: NotePlace;
  readonly at: NotePoint;
  /** False for a note past its place's last slot: it is counted, and it travels, but it is not drawn. */
  readonly shown: boolean;
}

export interface CardNoteMotion {
  readonly note: CardNote;
  readonly from: NotePoint;
  readonly startedAt: number | null;
}

/** The office's note anchors: how many slots each place has, a point for each slot, and which agents have a desk. */
export interface NoteAnchors {
  readonly desks: ReadonlySet<string>;
  readonly limit: (place: NotePlace) => number;
  readonly at: (place: NotePlace, slot: number) => NotePoint;
}
const TRAVEL_MS = 400;
const PLACES = { todo: 'board', claimed: 'desk', doing: 'desk', review: 'desk', blocked: 'board', done: 'archive' } as const;

export function placeCardNotes(cards: readonly CardRow[], anchors: NoteAnchors): readonly CardNote[] {
  const counts = new Map<string, number>();
  return [...cards].sort((a, b) => a.id.localeCompare(b.id)).map((card) => {
    const destination = PLACES[card.status];
    const place: NotePlace = destination === 'archive' ? { kind: 'archive' }
      : destination === 'desk' && card.claimed_by !== null && anchors.desks.has(card.claimed_by)
        ? { kind: 'desk', agentId: card.claimed_by } : { kind: 'board' };
    const key = place.kind === 'desk' ? place.agentId : place.kind;
    const slot = counts.get(key) ?? 0;
    counts.set(key, slot + 1);
    return { card, place, at: anchors.at(place, slot), shown: slot < anchors.limit(place) };
  });
}

function samePlace(before: NotePlace, after: NotePlace): boolean {
  return before.kind === after.kind && (before.kind !== 'desk' || after.kind === 'desk' && before.agentId === after.agentId);
}

export function planCardNotes(
  before: readonly CardNoteMotion[], next: readonly CardNote[], source: 'snapshot' | 'delta', now: number,
): readonly CardNoteMotion[] {
  const previous = new Map(before.map((motion) => [motion.note.card.id, motion]));
  return next.map((note) => {
    const prior = previous.get(note.card.id);
    if (source === 'snapshot' || prior === undefined) return { note, from: note.at, startedAt: null };
    if (samePlace(prior.note.place, note.place)) return { ...prior, note };
    return { note, from: pointAt(prior, now), startedAt: now };
  });
}

function pointAt(motion: CardNoteMotion, now: number): NotePoint {
  if (motion.startedAt === null) return motion.note.at;
  const progress = Math.max(0, Math.min(1, (now - motion.startedAt) / TRAVEL_MS));
  const target = motion.note.at;
  if (progress === 0) return motion.from;
  if (progress === 1) return target;
  return {
    x: motion.from.x + (target.x - motion.from.x) * progress,
    y: motion.from.y + (target.y - motion.from.y) * progress,
    z: motion.from.z + (target.z - motion.from.z) * progress,
  };
}

export function cardNotesAt(motions: readonly CardNoteMotion[], now: number, reducedMotion: boolean): readonly CardNote[] {
  return motions.map((motion) => ({ ...motion.note, at: reducedMotion ? motion.note.at : pointAt(motion, now) }));
}
