// SPDX-License-Identifier: MIT OR Apache-2.0

import { decodeActivity } from '../activity.ts';
import { decodeRetiredAgents } from '../archive.ts';
import type { FloorState } from './store.ts';

interface ReadAvailability {
  readonly epoch: number;
  readonly activity: boolean;
  readonly archived: boolean;
}

let availability: ReadAvailability = Object.freeze({ epoch: 0, activity: true, archived: true });
const listeners = new Set<() => void>();

export function getReadAvailability(): ReadAvailability { return availability; }
export function subscribeToReadAvailability(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function observeReadFrame(frame: Record<string, unknown>, floor: FloorState, previousLsn: number): void {
  if (floor.snapshotEpoch !== floor.epoch || floor.phase !== 'live') return;
  const snapshot = frame.type === 'floor.snapshot';
  if (!snapshot && (frame.type !== 'floor.delta' || floor.lsn <= previousLsn)) return;
  const value = snapshot ? frame.projection : frame.delta;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return;
  const rows = value as Record<string, unknown>;
  const before = availability.epoch === floor.epoch ? availability : { epoch: floor.epoch, activity: true, archived: true };
  const next = {
    epoch: floor.epoch,
    activity: !snapshot && rows.activity === undefined ? before.activity
      : Array.isArray(rows.activity) && decodeActivity(rows.activity).rejected === 0,
    archived: !snapshot && rows.retired_agents === undefined ? before.archived
      : Array.isArray(rows.retired_agents) && decodeRetiredAgents(rows.retired_agents).rejected === 0,
  };
  if (next.epoch === availability.epoch && next.activity === availability.activity && next.archived === availability.archived) return;
  availability = Object.freeze(next);
  for (const listener of [...listeners]) listener();
}
