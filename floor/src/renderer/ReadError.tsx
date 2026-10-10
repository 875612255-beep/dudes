// SPDX-License-Identifier: MIT OR Apache-2.0

import type { FloorState } from './floor/store.ts';
import { refreshFloorSnapshot } from './floor/stream.ts';

export type ReadState = 'loading' | 'ready' | 'unavailable' | 'error';

export function snapshotReadState(floor: FloorState, available = true): ReadState {
  if (floor.phase === 'closed') return 'unavailable';
  if (floor.phase !== 'live' || floor.snapshotEpoch !== floor.epoch) return 'loading';
  return available ? 'ready' : 'error';
}

export function ReadError({ message, onRetry = refreshFloorSnapshot, className = 'read-error' }: {
  readonly message: string;
  readonly onRetry?: () => void;
  readonly className?: string;
}) {
  return <p className={className} role="alert">{message} <button type="button" onClick={onRetry}>Retry</button></p>;
}

export function ReadNotice({ state, label, message }: {
  readonly state: ReadState;
  readonly label: string;
  readonly message: string;
}) {
  if (state === 'ready') return null;
  if (state === 'loading') return <p role="status">Waiting for a snapshot from the daemon. Loading {label}.</p>;
  return <ReadError message={state === 'unavailable'
    ? `${label} is unavailable while the stream is disconnected. Retry requests a snapshot with this session.`
    : message} />;
}

export function StreamReadStatus({ floor }: { readonly floor: FloorState }) {
  const state = snapshotReadState(floor);
  const label = state === 'unavailable' ? 'Disconnected — no current view'
    : floor.phase === 'resyncing' ? 'Resyncing — waiting for a snapshot'
      : state === 'loading' ? 'Connecting — waiting for a snapshot'
        : floor.stale ? 'View unconfirmed — waiting for the stream' : 'Live — confirmed';
  return <p className="stream-state" role="status">{label}</p>;
}
