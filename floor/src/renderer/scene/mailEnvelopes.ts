// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ProjectionEvent } from '../floor/projection.ts';
import { MAX_MESSAGES } from '../messages.ts';
import type { PlanPoint } from '../office3d/layout.ts';

const TRAVEL_MS = 3_000;

export interface EnvelopeMotion {
  readonly id: string;
  readonly from: PlanPoint;
  readonly to: PlanPoint;
  readonly startedAt: number;
}

/** One envelope in flight: where it is over the floor plan, and how far up its arc, from 0 at either desk to 1 midway. */
export interface Envelope {
  readonly id: string;
  readonly point: PlanPoint;
  readonly lift: number;
}

export function planEnvelopes(
  before: readonly EnvelopeMotion[], event: ProjectionEvent, desks: ReadonlyMap<string, PlanPoint>,
  now: number, reducedMotion: boolean,
): readonly EnvelopeMotion[] {
  if (event.kind !== 'delta' || event.previousMessagesLsn === null || event.floor.messagesLsn === null
    || event.floor.phase !== 'live' || event.floor.snapshotEpoch !== event.floor.epoch || reducedMotion) return [];
  const rows = event.floor.messages;
  const ids = new Set(rows.filter(row => desks.has(row.from) && desks.has(row.to)).map(row => row.id));
  const active = before.filter(motion => now - motion.startedAt < TRAVEL_MS && ids.has(motion.id));
  const activeIds = new Set(active.map(motion => motion.id));
  for (const row of rows) {
    if (BigInt(row.lsn) <= BigInt(event.previousMessagesLsn) || activeIds.has(row.id)) continue;
    const from = desks.get(row.from);
    const to = desks.get(row.to);
    if (from === undefined || to === undefined) continue;
    active.push({ id: row.id, from, to, startedAt: now });
    activeIds.add(row.id);
  }
  return active.slice(-MAX_MESSAGES);
}

export function activeEnvelopes(
  motions: readonly EnvelopeMotion[], now: number, reducedMotion: boolean,
): readonly EnvelopeMotion[] {
  return reducedMotion ? [] : motions.filter(motion => now - motion.startedAt < TRAVEL_MS);
}

export function envelopesAt(motions: readonly EnvelopeMotion[], now: number): readonly Envelope[] {
  return motions.map(motion => {
    const progress = Math.max(0, Math.min(1, (now - motion.startedAt) / TRAVEL_MS));
    return { id: motion.id, lift: Math.sin(Math.PI * progress),
      point: { x: motion.from.x + (motion.to.x - motion.from.x) * progress, y: motion.from.y + (motion.to.y - motion.from.y) * progress } };
  });
}
