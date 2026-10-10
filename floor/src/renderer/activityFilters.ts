// SPDX-License-Identifier: MIT OR Apache-2.0

import { activityText } from './activity.ts';
import type { ActivityRow } from './activity.ts';
import type { ActivityFilters } from './view.ts';

export function filterActivity(rows: readonly ActivityRow[], filters: ActivityFilters): readonly ActivityRow[] {
  const query = filters.query.trim().toLowerCase();
  return rows.filter((row) => (filters.agentId === '' || ('agent' in row && row.agent.id === filters.agentId))
    && (filters.kind === '' || row.kind === filters.kind)
    && (query === '' || activityText(row).toLowerCase().includes(query)));
}
