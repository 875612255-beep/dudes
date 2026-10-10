// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Who is in the office, and how they came or go. A hire walks in through the
// door and a retired agent walks out through it; a first load or a resync
// places everyone at once, so nobody parades in or out. Only the roster the
// Floor already reads drives it, plus the end of each walk. `motion.ts` plans
// the walks themselves.

import type { PlanPoint } from '../office3d/layout.ts';

/** `gone` is the last step: the figure walked out and leaves the scene. */
export type Presence = 'entering' | 'present' | 'leaving' | 'gone';

export interface Transition {
  readonly id: string;
  /** `null` for an agent the office was not drawing. */
  readonly from: Presence | null;
  readonly to: Presence;
}

/** Every agent the office draws, by AgentId. A gone agent is off it. */
export type Roster = ReadonlyMap<string, Exclude<Presence, 'gone'>>;

const AFTER_WALK: Readonly<Partial<Record<Presence, Presence>>> = Object.freeze({ entering: 'present', leaving: 'gone' });

/**
 * What one roster update does. On a delta, an id new to the office enters and
 * one missing from the roster leaves. A snapshot is a first load or a resync,
 * so every id on it is present at once and anyone missing is gone.
 */
export function rosterTransitions(previous: Roster, next: readonly string[], update: 'snapshot' | 'delta'): readonly Transition[] {
  const onRoster = new Set(next);
  const transitions: Transition[] = [];
  for (const [id, from] of previous) {
    if (onRoster.has(id)) continue;
    if (update === 'snapshot') transitions.push({ id, from, to: 'gone' });
    else if (from !== 'leaving') transitions.push({ id, from, to: 'leaving' });
  }
  for (const id of next) {
    const from = previous.get(id) ?? null;
    const to = update === 'snapshot' ? 'present' : from === null ? 'entering' : from === 'leaving' ? 'present' : from;
    if (to !== from) transitions.push({ id, from, to });
  }
  return transitions;
}

/** The walks that ended: an agent that came in through the door is present, and one that went out of it is gone. */
export function arrivalTransitions(previous: Roster, arrived: (id: string) => boolean): readonly Transition[] {
  const transitions: Transition[] = [];
  for (const [id, from] of previous) {
    const to = AFTER_WALK[from];
    if (to !== undefined && arrived(id)) transitions.push({ id, from, to });
  }
  return transitions;
}

/**
 * A hire waits outside until the doorway is clear, and the office draws it once it steps in. Only a walk that
 * still starts at the door is outside: a hire re-planned after it stepped in waits where it stands.
 */
export function inOffice(presence: Presence | undefined, walk: { readonly travel: readonly PlanPoint[]; readonly startedAt: number },
  door: PlanPoint, now: number): boolean {
  const [from] = walk.travel;
  return presence !== 'entering' || now >= walk.startedAt || from.x !== door.x || from.y !== door.y;
}

export function applyTransitions(roster: Roster, transitions: readonly Transition[]): Roster {
  const next = new Map(roster);
  for (const { id, to } of transitions) {
    if (to === 'gone') next.delete(id);
    else next.set(id, to);
  }
  return next;
}
