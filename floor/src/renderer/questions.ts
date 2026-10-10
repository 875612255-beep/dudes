// SPDX-License-Identifier: MIT OR Apache-2.0

import { canonicalLsn } from './answer/answer.ts';
import type { AgentRow } from './scene/cast.ts';
import { fieldsOf } from './scene/label.ts';

export interface OpenQuestion {
  readonly row: AgentRow;
  readonly name: string;
  readonly text: string;
  readonly lsn: bigint | null;
}

export function openQuestions(agents: readonly AgentRow[]): readonly OpenQuestion[] {
  return agents.flatMap((row): OpenQuestion[] => {
    const text = canonicalLsn(row.question_lsn) && typeof row.question === 'string'
      ? row.question : fieldsOf(row).question;
    if (text === '') return [];
    const name = typeof row.name === 'string' && row.name !== '' ? row.name : row.id;
    return [{ row, name, text, lsn: canonicalLsn(row.question_lsn) ? BigInt(row.question_lsn) : null }];
  }).sort(newestFirstUnplacedLast);
}

function newestFirstUnplacedLast(a: OpenQuestion, b: OpenQuestion): number {
  if (a.lsn === b.lsn) return 0;
  if (a.lsn === null) return 1;
  if (b.lsn === null) return -1;
  return a.lsn > b.lsn ? -1 : 1;
}
