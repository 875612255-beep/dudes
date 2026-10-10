// SPDX-License-Identifier: MIT OR Apache-2.0

import { activityText } from './activity.ts';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { filterActivity } from './activityFilters.ts';
import { readActivityFilters, writeActivityFilters } from './view.ts';
import { ReadNotice, snapshotReadState } from './ReadError.tsx';
import { getReadAvailability, subscribeToReadAvailability } from './floor/reads.ts';
import { journalPosition, recordedTime } from './format.ts';
import type { FloorState } from './floor/store.ts';

export function LogPanel({ floor }: { readonly floor: FloorState }) {
  const [filters, setFilters] = useState(readActivityFilters);
  const reads = useSyncExternalStore(subscribeToReadAvailability, getReadAvailability, getReadAvailability);
  const state = snapshotReadState(floor, reads.epoch !== floor.epoch || reads.activity);
  const agents = new Map<string, string>();
  for (const row of floor.activity) {
    if ('agent' in row) agents.set(row.agent.id, row.agent.name || row.agent.id);
  }
  if (filters.agentId !== '' && !agents.has(filters.agentId)) agents.set(filters.agentId, filters.agentId);
  const kinds = new Set<string>(floor.activity.map((row) => row.kind));
  if (filters.kind !== '') kinds.add(filters.kind);
  const rows = filterActivity([...floor.activity].reverse(), filters);

  useEffect(() => { writeActivityFilters(filters); }, [filters]);

  return (
    <section className="log-panel" aria-label="Recent activity">
      <ReadNotice state={state} label="Activity" message="Activity was not reported, or this page could not read every event. Retry to request a fresh snapshot." />
      {state !== 'ready' ? null : <p role="status">{floor.stale ? 'Activity unconfirmed. These events are from the last snapshot.' : 'Recent journal events, newest first.'}</p>}
      <div className="activity-filters" role="group" aria-label="Filter recent activity">
        <label>Agent
          <select value={filters.agentId} onChange={(event) => setFilters({ ...filters, agentId: event.currentTarget.value })}>
            <option value="">All agents</option>
            {[...agents].map(([id, name]) => <option value={id} key={id}>{name}</option>)}
          </select>
        </label>
        <label>Event kind
          <select value={filters.kind} onChange={(event) => setFilters({ ...filters, kind: event.currentTarget.value })}>
            <option value="">All kinds</option>
            {[...kinds].sort().map((kind) => <option value={kind} key={kind}>{kind.replaceAll('_', ' ')}</option>)}
          </select>
        </label>
        <label>Search
          <input type="search" value={filters.query} maxLength={128} placeholder="Search activity"
            onChange={(event) => setFilters({ ...filters, query: event.currentTarget.value })} />
        </label>
      </div>
      {rows.length === 0 ? state === 'ready' ? <p>{floor.activity.length === 0 ? 'No recent activity.' : 'No matching activity.'}</p> : null : (
        <ol aria-label="Recent journal events">
          {rows.map((row) => (
            <li key={row.lsn} data-activity-lsn={row.lsn}>
              <p>{activityText(row)}</p>
              <small>{recordedTime(row.at)} · {journalPosition(row.lsn)}</small>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
