// SPDX-License-Identifier: MIT OR Apache-2.0

import type { AgentRow } from './scene/cast.ts';

export interface DoneNotice {
  readonly key: string;
  readonly agentId: string;
  readonly name: string;
  readonly goal: string;
}

function doneGoal(row: AgentRow): { readonly id: string; readonly text: string } | null {
  if (typeof row.goal !== 'object' || row.goal === null) return null;
  const { id, text, status } = row.goal as Record<string, unknown>;
  return status === 'achieved' && typeof id === 'string' ? { id, text: typeof text === 'string' ? text : '' } : null;
}

export function newlyDone(before: readonly AgentRow[], after: readonly AgentRow[]): readonly DoneNotice[] {
  const was = new Map(before.map((row) => [row.id, doneGoal(row)?.id]));
  return after.flatMap((row): DoneNotice[] => {
    const goal = doneGoal(row);
    if (goal === null || was.get(row.id) === goal.id) return [];
    const name = typeof row.name === 'string' && row.name !== '' ? row.name : row.id;
    return [{ key: `${row.id}-${goal.id}`, agentId: row.id, name, goal: goal.text }];
  });
}
