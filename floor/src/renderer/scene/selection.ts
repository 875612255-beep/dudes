// SPDX-License-Identifier: MIT OR Apache-2.0

import type { AgentRow } from './cast.ts';

/** Resolve a selection against the current snapshot, never a saved row. */
export function selectedAgentRow(
  agents: readonly AgentRow[],
  selectedId: string | null,
  phase: string,
  stale: boolean,
): AgentRow | null {
  if (selectedId === null || stale || phase === 'closed' || phase === 'resyncing') return null;
  return agents.find((agent) => agent.id === selectedId) ?? null;
}
