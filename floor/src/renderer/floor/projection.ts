// SPDX-License-Identifier: MIT OR Apache-2.0

import type { FloorState } from './store.ts';

export interface ProjectionEvent {
  readonly kind: 'snapshot' | 'delta';
  readonly snapshotGeneration: number;
  readonly floor: FloorState;
  readonly previousMessagesLsn: string | null;
}

let snapshotGeneration = 0;
const listeners = new Set<(event: ProjectionEvent) => void>();

export function subscribeToProjection(listener: (event: ProjectionEvent) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function publishProjection(
  kind: ProjectionEvent['kind'], floor: FloorState, previousMessagesLsn: string | null = null,
): void {
  if (kind === 'snapshot') snapshotGeneration += 1;
  const event = { kind, snapshotGeneration, floor, previousMessagesLsn };
  for (const listener of listeners) listener(event);
}
