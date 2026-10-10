// SPDX-License-Identifier: MIT OR Apache-2.0

import { canonicalLsn } from './answer/answer.ts';

const ID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const UTF8 = new TextEncoder();
const FIELDS = ['agent_id', 'question_lsn', 'answer_lsn', 'question'];
export const MAX_QUESTION_HISTORY = 64;
export const MAX_AGENT_QUESTION_HISTORY = 8;
export const MAX_QUESTION_HISTORY_BYTES = 32_768;

export interface AnsweredQuestion {
  readonly agent_id: string;
  readonly question_lsn: string;
  readonly answer_lsn: string;
  readonly question: string;
}

export function decodeQuestionHistory(value: unknown): {
  readonly rows: readonly AnsweredQuestion[];
  readonly rejected: number;
} {
  const empty = (rejected: number) => ({ rows: Object.freeze([]), rejected });
  if (value === undefined) return empty(0);
  if (!Array.isArray(value)) return empty(1);
  if (value.length > MAX_QUESTION_HISTORY || UTF8.encode(JSON.stringify(value)).length > MAX_QUESTION_HISTORY_BYTES) {
    return empty(value.length || 1);
  }
  const rows: AnsweredQuestion[] = [];
  const counts = new Map<string, number>();
  const questions = new Set<string>();
  let last = 0n;
  for (const raw of value) {
    if (typeof raw !== 'object' || raw === null || Array.isArray(raw)
      || Object.keys(raw).length !== FIELDS.length || !FIELDS.every((field) => Object.hasOwn(raw, field))) return empty(value.length || 1);
    const source = raw as Record<string, unknown>;
    if (typeof source.agent_id !== 'string' || !ID.test(source.agent_id)
      || !canonicalLsn(source.question_lsn) || !canonicalLsn(source.answer_lsn)
      || BigInt(source.question_lsn) >= BigInt(source.answer_lsn) || BigInt(source.answer_lsn) <= last
      || typeof source.question !== 'string') return empty(value.length || 1);
    const count = (counts.get(source.agent_id) ?? 0) + 1;
    const key = `${source.agent_id}/${source.question_lsn}`;
    if (count > MAX_AGENT_QUESTION_HISTORY || questions.has(key)) return empty(value.length || 1);
    counts.set(source.agent_id, count);
    questions.add(key);
    last = BigInt(source.answer_lsn);
    rows.push(Object.freeze({ agent_id: source.agent_id, question_lsn: source.question_lsn,
      answer_lsn: source.answer_lsn, question: source.question }));
  }
  return { rows: Object.freeze(rows), rejected: 0 };
}
